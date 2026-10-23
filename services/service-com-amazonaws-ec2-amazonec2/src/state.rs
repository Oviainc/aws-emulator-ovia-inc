// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ec2-amazonec2 service.
//!
//! This module owns the EC2 instance lifecycle: the reservations `RunInstances`
//! mints, the instances inside them, their tags, and the clock-owed state
//! transitions `Start`/`Stop`/`TerminateInstances` start. The projection of a
//! stored instance into the modelled `Instance` shape lives here too, so
//! `RunInstances` and `DescribeInstances` cannot report the same instance
//! differently.
//!
//! Each operation's own validation and orchestration stays in its
//! `src/ops/<operation>.rs` file; only the data model and the primitives several
//! operations share live here.
//!
//! Every resource is stored as the MODELLED shape the describe operations return,
//! beside the few facts those shapes carry no member for, keyed by the same resource
//! id ([`ComAmazonawsEc2Amazonec2State::vpc_available_at`] is when a `pending` VPC's
//! `available` transition falls due, on the same monotonic clock and the same
//! [`TRANSITION_DELAY`] as an instance's [`PendingTransition`]). This module also holds what several operations
//! share: the resource-id mint, the CIDR canonicalisation the `CidrBlock` members
//! document, the Availability Zone facts, the EC2 filter-value matcher, the page-token
//! codec, and the EC2 error channel (EC2's model declares no error shapes at all, so
//! every documented error code is returned through [`crate::scaffolded`]).
//!
//! ## Availability Zones are facts of a Region, not stored resources
//!
//! Nothing in the EC2 API creates, renames or deletes an Availability Zone: AWS
//! reports the zones the Region a request was made in *has*. So this module derives
//! a Region's zone set from the Region name — the one thing a request says about the
//! Region it is asking in, in its SigV4 credential scope — instead of keeping a copy
//! of it in [`ComAmazonawsEc2Amazonec2State`]. `DescribeAvailabilityZones` therefore
//! writes nothing at all: two calls signed for the same Region report the same zones,
//! and a read can never change what a later caller sees.
//!
//! The instance types a location offers are facts of that location in exactly the
//! same way — no EC2 operation creates or withdraws an offering — so
//! [`instance_type_offerings`] derives them from the Region and the modelled
//! `InstanceType` catalog, and `DescribeInstanceTypeOfferings` writes nothing either.
//!
//! ## Resources callers create are stored
//!
//! [`ComAmazonawsEc2Amazonec2State::vpcs`] holds the account's VPCs as the modelled
//! [`Vpc`] shape itself rather than a parallel record, so every member a writer sets
//! round-trips through `DescribeVpcs` without a field-by-field copy that can silently
//! drop one. The store is keyed by `VpcId` in a [`BTreeMap`], which gives the describe
//! path one stable order and lets a pagination token name the id the next page starts
//! at. [`ComAmazonawsEc2Amazonec2State::subnets`] holds the account's subnets the same
//! way, keyed by `SubnetId`. [`ComAmazonawsEc2Amazonec2State::security_groups`] holds the
//! account's security groups the same way, keyed by `GroupId`, and
//! [`ComAmazonawsEc2Amazonec2State::images`] the images it can see, keyed by `ImageId`.
//! Further caller-created resources belong beside them as they are implemented, and should
//! be validated against [`availability_zones_in_region`] rather than against a second zone
//! list.
//!
//! A fact of a VPC that the modelled [`Vpc`] shape carries no member for is stored beside
//! it under the same id rather than on it:
//! [`ComAmazonawsEc2Amazonec2State::vpc_attributes`] holds the `VpcAttributeName`
//! attributes `ModifyVpcAttribute` sets and `DescribeVpcAttribute` reports, so those two
//! operations and `CreateVpc`'s defaults cannot disagree about one VPC.
//!
//! ## Windows fast launch
//!
//! [`ComAmazonawsEc2Amazonec2State::fast_launch_images`] holds the Windows fast launch
//! configuration of each image that has one, keyed by the `ImageId` it belongs to, as
//! [`FastLaunchRecord`]s: the modelled [`DescribeFastLaunchImagesSuccessItem`] every
//! operation that reports one answers with, plus the move the clock owes it
//! ([`FastLaunchTransition`]) — `enabling` -> `enabled` after `EnableFastLaunch`, and the
//! removal of the configuration after `DisableFastLaunch`, since `FastLaunchStateCode` has
//! no word for "not enabled" and an image whose pre-provisioned snapshots are gone is
//! simply not a fast launch enabled image any more. Both fall due on the same monotonic
//! clock and the same [`TRANSITION_DELAY`] as an instance's [`PendingTransition`], and
//! [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] applies them.
//!
//! A configuration is keyed by an image of [`ComAmazonawsEc2Amazonec2State::images`], so
//! the store cannot hold more configurations than there are images, and every string one
//! retains is either a word the service chose or an id read back out of another store —
//! nothing caller-sized — so no byte charge is made for one.
//!
//! ## Launch templates
//!
//! [`ComAmazonawsEc2Amazonec2State::launch_templates`] holds the account's launch
//! templates as [`LaunchTemplateRecord`]s: the modelled [`LaunchTemplate`] every operation
//! that reports one answers with, plus the versions of the template, because "deleting a
//! launch template deletes all of its versions" (`DeleteLaunchTemplate`) — so the versions
//! have to be part of the one record the delete removes. The store is keyed by
//! `LaunchTemplateId` in a [`BTreeMap`], the same stable order the VPC and subnet stores
//! are kept in, and a template is reached by name through
//! [`ComAmazonawsEc2Amazonec2State::launch_template_id_for_name`] because the API lets a
//! caller name either.
//!
//! ## EBS volumes
//!
//! [`ComAmazonawsEc2Amazonec2State::volumes`] holds the account's EBS volumes as
//! [`VolumeRecord`]s: the modelled [`VolumeView`] (`com.amazonaws.ec2#Volume`) every
//! operation that reports one answers with, plus the instant the clock owes the volume its
//! `creating` -> `available` move — the one member of the view that is a fact of the
//! volume's age rather than of the request, and so derived on read instead of stored.
//! Keyed by `VolumeId` in a [`BTreeMap`], the same stable order the VPC and subnet stores
//! are kept in, so a pagination token can name the id the next page starts at.
//!
//! ## EBS volume modifications
//!
//! [`ComAmazonawsEc2Amazonec2State::volume_modifications`] holds, per `VolumeId`, the most
//! recent modification of that volume as a [`VolumeModificationRecord`]: the modelled
//! [`VolumeModification`] every operation that reports one answers with, plus the instants
//! the clock owes it its `modifying` -> `optimizing` -> `completed` moves. The state, the
//! progress and the end time are facts of the modification's age rather than of the request
//! that started it, so [`VolumeModificationRecord::view_at`] derives all three on read
//! instead of storing a word two writers could disagree about — the same arrangement
//! [`VolumeRecord`] uses for a volume's own `State`, on the same monotonic clock and the
//! same [`TRANSITION_DELAY`].
//!
//! One volume has at most ONE record: "Describes the most recent volume modification
//! request for the specified EBS volumes" (`DescribeVolumesModifications`), so a later
//! modification of the same volume replaces the record rather than adding one beside it.
//!
//! ## Verified Access trust providers
//!
//! [`ComAmazonawsEc2Amazonec2State::verified_access_trust_providers`] holds the account's
//! Verified Access trust providers as the modelled
//! [`crate::ops::types::VerifiedAccessTrustProvider`] shape itself — the one
//! `CreateVerifiedAccessTrustProvider` answers with and
//! `DescribeVerifiedAccessTrustProviders` reports — keyed by
//! `VerifiedAccessTrustProviderId` in a [`BTreeMap`], the same stable order the VPC and
//! subnet stores are kept in, so a pagination token can name the id the next page starts
//! at. The shape carries no status member: AWS hands a trust provider back ready, so the
//! clock owes it no transition and there is nothing to derive on read.
//!
//! ## Secondary networks
//!
//! [`ComAmazonawsEc2Amazonec2State::secondary_networks`] holds the account's secondary
//! networks as [`SecondaryNetworkRecord`]s, the same shape of record the volume store
//! keeps: the modelled [`SecondaryNetwork`] every operation that reports one answers with,
//! plus the instant the clock owes the network its `create-in-progress` ->
//! `create-complete` move and, once a delete has asked for it, the instant it owes it its
//! `delete-in-progress` -> `delete-complete` move. The network's `State` and the `State` of
//! each of its IPv4 CIDR block associations are facts of its age rather than of the
//! request, so [`SecondaryNetworkRecord::view_at`] derives both on read instead of storing
//! them; a deleted network keeps its record, because `DescribeSecondaryNetworks` reports
//! the delete states too. Keyed
//! by `SecondaryNetworkId` in a [`BTreeMap`], the same stable order the VPC, subnet and
//! volume stores are kept in, so a pagination token can name the id the next page starts
//! at.
//!
//! ## BYOIP address ranges
//!
//! [`ComAmazonawsEc2Amazonec2State::byoip_cidrs`] holds the address ranges the account has
//! brought to AWS as [`ByoipCidrRecord`]s: the modelled [`ByoipCidr`] every operation that
//! reports one answers with, plus the release the clock owes a range a
//! `DeprovisionByoipCidr` has started ([`ByoipCidrRecord::released_at`], on the same monotonic
//! clock and the same [`TRANSITION_DELAY`] as a `pending` VPC's `available` move). Keyed by
//! the range itself in its canonical form ([`canonical_byoip_cidr`]), because the range IS
//! the name the API takes — every BYOIP operation is addressed by `Cidr`, not by a minted id
//! — which also gives the describe path one stable order. That key comes from a REQUEST, so
//! the store is bounded like every other one here: a new range has to fit inside the record
//! ceiling ([`ComAmazonawsEc2Amazonec2State::has_byoip_cidr_capacity`]) and the byte budget,
//! and [`byoip_cidr_payload_bytes`] is the one definition of what a range is charged, given
//! back when the clock's release drops it.
//!
//! ## Elastic IP addresses
//!
//! [`ComAmazonawsEc2Amazonec2State::addresses`] holds the account's Elastic IP addresses
//! as the modelled [`Address`] shape itself, keyed by `AllocationId`, for the same reason
//! the VPC store holds [`Vpc`]s: `AllocateAddress` and `DescribeAddresses` cannot report
//! one address differently. The address itself is minted out of Amazon's pool by
//! [`ComAmazonawsEc2Amazonec2State::mint_elastic_ip`], never taken from the request, and
//! an allocation is reached by its public address through
//! [`ComAmazonawsEc2Amazonec2State::allocation_id_for_public_ip`] because the API lets a
//! caller name either. An Elastic IP address AWS hands back is usable at once — it has no
//! non-terminal status — so no entry beside it records a clock-owed transition.
//!
//! An address's ASSOCIATION with one of the account's resources lives on that same record
//! — `Address$AssociationId`, `Address$InstanceId` and `Address$PrivateIpAddress` — rather
//! than in a map beside it, so `AssociateAddress` and `DescribeAddresses` cannot disagree
//! about what an address is attached to. The instance reports the same address through
//! `Instance$PublicIpAddress`, so the ONE primitive that ends an association
//! ([`ComAmazonawsEc2Amazonec2State::detach_elastic_ip`]) clears both sides, and
//! [`ComAmazonawsEc2Amazonec2State::allocation_id_associated_with_instance`] answers which
//! address an instance holds.
//!
//! ## Dedicated Hosts
//!
//! [`ComAmazonawsEc2Amazonec2State::dedicated_hosts`] holds the account's Dedicated Hosts
//! as the modelled [`Host`] shape itself, keyed by `HostId`, for the same reason the VPC
//! store holds [`Vpc`]s: `AllocateHosts` and the describe operations cannot report one
//! host differently. Which of those hosts is an EC2 Mac Dedicated Host is a fact of the
//! host's instance family rather than a stored flag ([`is_mac_instance_family`]), and the
//! macOS releases one can launch are a fact of that family's hardware
//! ([`mac_os_supported_versions`]) — so [`mac_host_view`] derives the `MacHost` shape
//! `DescribeMacHosts` reports from the one stored host, and no second record can disagree
//! with it.
//!
//! ## VPC endpoint connections
//!
//! [`ComAmazonawsEc2Amazonec2State::vpc_endpoint_connections`] holds the connections to
//! the account's OWN VPC endpoint services — "the VPC endpoint connections to your VPC
//! endpoint services" (`DescribeVpcEndpointConnections`) — as the modelled
//! [`VpcEndpointConnection`] shape itself, keyed by `VpcEndpointId` in a [`BTreeMap`], for
//! the same reasons the VPC store holds [`Vpc`]s: one endpoint connects to exactly one
//! service, so the endpoint id identifies the connection, every member a writer sets
//! round-trips through the describe without a field-by-field copy, and the map's order is
//! the one order the describe reports and pages over. A fresh account has created no
//! endpoint service, so it has no connection either and the store starts empty.
//! [`ComAmazonawsEc2Amazonec2State::insert_vpc_endpoint_connection`] is the one way a
//! connection is stored, so the id it carries, the bytes it charges and the ceiling it
//! fits inside cannot be decided two ways: a connection is keyed by a REQUEST-SUPPLIED
//! endpoint id and nothing here reclaims one, so the insert refuses past
//! [`MAX_STORED_VPC_ENDPOINT_CONNECTIONS`] (a writer asks
//! [`ComAmazonawsEc2Amazonec2State::has_vpc_endpoint_connection_capacity`] first and
//! refuses with EC2's own quota code) and every record costs at least
//! [`VPC_ENDPOINT_CONNECTION_RECORD_BYTES`] against the byte budget, so repeated writes
//! cannot grow the store for free. A connection sits in `PendingAcceptance` until the
//! SERVICE OWNER accepts or rejects it (`AcceptVpcEndpointConnections` /
//! `RejectVpcEndpointConnections`), which is an action rather than an instant, so —
//! unlike a `pending` VPC — nothing beside it records a clock-owed transition until the
//! owner acts: once `AcceptVpcEndpointConnections` has, the connection is `Pending` and
//! [`ComAmazonawsEc2Amazonec2State::vpc_endpoint_connection_moves`] holds the instant the
//! clock owes it `Available`.
//!
//! That pair of bounds — a count ceiling the writer itself re-checks, and a per-record byte
//! floor so a record carrying no tags is not free — is the rule for EVERY store nothing in
//! this emulator reclaims, not just this one. The writers of the transit gateway
//! attachments, multicast domains and route table announcements, the VPC endpoint services
//! and endpoints, and the IPAM pools answer `None` past [`MAX_STORED_RECORDS_PER_STORE`]
//! and charge [`STORED_RECORD_OVERHEAD_BYTES`] on top of the caller-chosen bytes they hold,
//! so repeated writes cannot grow this process's memory without bound whatever the records
//! carry. The Allowed AMIs settings are bounded the same way
//! ([`MAX_ALLOWED_IMAGES_SETTINGS_REGIONS`] and [`image_criteria_payload_bytes`]).
//!
//! ## VPC endpoints, endpoint services, transit gateway attachments and IPAM pools
//!
//! Each of these is held as the modelled shape its own operations report —
//! [`ComAmazonawsEc2Amazonec2State::vpc_endpoints`],
//! [`ComAmazonawsEc2Amazonec2State::vpc_endpoint_services`],
//! [`ComAmazonawsEc2Amazonec2State::transit_gateway_attachments`],
//! [`ComAmazonawsEc2Amazonec2State::transit_gateway_multicast_domains`],
//! [`ComAmazonawsEc2Amazonec2State::transit_gateway_route_table_announcements`] and
//! [`ComAmazonawsEc2Amazonec2State::ipam_pools`] — keyed in a [`BTreeMap`] by the id that
//! names it, for the same reasons the VPC store
//! holds [`Vpc`]s. A fresh account holds none of them: every one is created by a request,
//! so each store starts empty, and the `insert_*` writer beside each is the ONE way a
//! record is stored so its id and its byte charge cannot be minted two ways. Where an
//! operation leaves a resource in a NON-TERMINAL state — an accepted attachment, an
//! announcement a delete started on, a range whose advertisement was asked for — the
//! record carries the [`PendingStateMove`] the clock owes it, which
//! [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] applies — the BYOIP range
//! above included, whose record carries one beside the release it may owe.
//!
//! ## VPC Block Public Access exclusions
//!
//! [`ComAmazonawsEc2Amazonec2State::vpc_block_public_access_exclusions`] holds the
//! account's VPC BPA exclusions as [`VpcBlockPublicAccessExclusionRecord`]s, keyed by
//! `ExclusionId` in a [`BTreeMap`] so the describe path has one stable order and a
//! pagination token can name the id the next page starts at. The record is the modelled
//! [`VpcBlockPublicAccessExclusion`] every operation that reports one answers with, plus
//! the two facts that shape carries no member for: the VPC or subnet the exclusion is
//! applied to ([`ExclusionTarget`] — "a mode that can be applied to a single VPC or
//! subnet"), and the move the clock owes it ([`ExclusionTransition`]), since
//! `CreateVpcBlockPublicAccessExclusion` answers an exclusion in the non-terminal
//! `create-in-progress` state and the clock owes it `create-complete` one
//! [`TRANSITION_DELAY`] later — the same span, off the same monotonic clock, as an
//! instance's [`PendingTransition`] and a `pending` VPC's `available` move, applied by
//! [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`].
//!
//! ## AMIs: the published catalog, plus whatever a writer stores
//!
//! [`ComAmazonawsEc2Amazonec2State::images`] holds the AMIs the account can see as the
//! modelled [`Image`] shape itself, keyed by `ImageId`, for the same reason the VPC store
//! holds [`Vpc`]s: every member a writer sets round-trips through `DescribeImages` without
//! a field-by-field copy that can silently drop one. Nothing in the EC2 API creates
//! Amazon's own AMIs — "the images available to you include public images"
//! (`DescribeImages`) — so the store starts with the public images Amazon publishes
//! ([`published_image_specs`]), the same way it starts with the default VPC AWS creates
//! for a fresh account, and an image a later operation registers goes beside them through
//! [`ComAmazonawsEc2Amazonec2State::insert_image`]. Each one's `CreationDate` is the build
//! date its published name carries, so it is the same across reads and across runs and the
//! newest image of a name pattern is always the same image.
//!
//! ## Allowed AMIs is an account-and-Region SETTING, not a resource
//!
//! [`ComAmazonawsEc2Amazonec2State::allowed_images_settings`] holds the Allowed AMIs
//! setting of the account "at the account level in the specified Region"
//! (`GetAllowedImagesSettings`) as an [`AllowedImagesSettings`], keyed by the Region the
//! request that wrote it was signed for ([`request_region`]) — the one thing a request
//! says about the Region it is asking in. It is not a resource: nothing mints or deletes
//! one, so a Region nobody has configured has NO entry and reports
//! [`AllowedImagesSettings::default`] — the `disabled` state with no criteria, which is
//! what an account that has never enabled the feature has. `GetAllowedImagesSettings`
//! therefore writes nothing at all, the way `DescribeAvailabilityZones` does.
//!
//! The two facts the setting carries move independently, which is why one record holds
//! both: `EnableAllowedImagesSettings` / `DisableAllowedImagesSettings` move the state and
//! leave the criteria alone (`GetAllowedImagesSettings` reports "the current state of the
//! Allowed AMIs setting AND the list of Allowed AMIs criteria" whichever state it is in),
//! and `ReplaceImageCriteriaInAllowedImagesSettings` replaces the criteria and leaves the
//! state alone. The criteria are stored as the modelled [`ImageCriterion`] shape the get
//! reports, so the request shapes a replace carries are converted once, where they are
//! validated, rather than at every read.
//!
//! ## The default VPC's default subnets
//!
//! "When we create a default VPC, we create a default subnet in each Availability Zone
//! in the Region" (Amazon VPC User Guide, "Default VPCs"), so the subnet store starts
//! with one default subnet per zone of the seeded default VPC rather than empty — the
//! same reason the VPC store starts with that VPC. This store is per ACCOUNT, not per
//! Region (a `Vpc` carries no Region member, so nothing in `DescribeVpcs` could say
//! which Region its VPC belongs to), and a subnet's zone and ARN do name a Region, so
//! the seeded subnets are the ones of [`DEFAULT_REGION`]. Everything a later writer
//! stores names the Region its own request was signed for.
//!
//! ## Account-level settings
//!
//! A setting is not a resource: it has no id, nothing creates or deletes it, and an
//! account that has never touched it still has a value for it. So
//! [`ComAmazonawsEc2Amazonec2State::snapshot_block_public_access`] is a single field
//! holding the one word the *block public access for snapshots* setting is in —
//! [`SNAPSHOT_BLOCK_PUBLIC_ACCESS_UNBLOCKED`] for an account that has never enabled it —
//! rather than a map keyed by anything. The value stored is always a member of the
//! modelled `SnapshotBlockPublicAccessState` catalog ([`snapshot_block_public_access_mode`]
//! is what turns a request's word into one), never the request's own text, and the field
//! is account-wide because this store is per ACCOUNT and not per Region (see "The default
//! VPC's default subnets" below).
//!
//! ## The default security group of a VPC
//!
//! "Your VPC automatically comes with a default security group. … you can't delete a
//! default security group" (Amazon VPC User Guide, "Default security groups for your
//! VPCs"), so [`ComAmazonawsEc2Amazonec2State::security_groups`] starts with the default
//! VPC's default group rather than empty: the ambient network a fresh account has is a
//! VPC, its per-zone default subnets AND that group, and a client provisioning into the
//! default VPC reads all three before it can name a network. The store holds the modelled
//! [`SecurityGroup`] shape itself, keyed by `GroupId` in a [`BTreeMap`], for the same
//! reasons the VPC and subnet stores do.
//!
//! ## Transit gateways
//!
//! [`ComAmazonawsEc2Amazonec2State::transit_gateways`] holds the account's transit
//! gateways as the modelled [`TransitGateway`] shape itself, keyed by `TransitGatewayId`,
//! for the same reasons the VPC store does — every member a writer sets round-trips
//! through `DescribeTransitGateways` without a field-by-field copy. The clock-owed
//! `pending` -> `available` move lives beside it in
//! [`ComAmazonawsEc2Amazonec2State::transit_gateway_available_at`], on the same monotonic
//! clock and the same [`TRANSITION_DELAY`] as a VPC's.
//!
//! The default transit gateway route table a create makes ("When you create a transit
//! gateway, we create a default transit gateway route table and use it as the default
//! association route table and the default propagation route table",
//! `CreateTransitGateway`) is a resource of its own, so it is stored as one in
//! [`ComAmazonawsEc2Amazonec2State::transit_gateway_route_tables`] rather than existing
//! only as an id inside the gateway's options: the ids
//! `TransitGatewayOptions$AssociationDefaultRouteTableId` and
//! `$PropagationDefaultRouteTableId` report name that record.
//!
//! Which attachment propagates its routes into which of those route tables is a resource of
//! its own too — "Enables the specified attachment to propagate routes to the specified
//! propagation route table" (`EnableTransitGatewayRouteTablePropagation`) — so
//! [`ComAmazonawsEc2Amazonec2State::transit_gateway_propagations`] holds one
//! [`TransitGatewayPropagationRecord`] per (route table, attachment) pair, which is exactly
//! as many as there can be: a pair either propagates or does not. The record carries the
//! `TransitGatewayPropagationState` reached so far plus the move the clock owes it
//! ([`PropagationMove`]) — `enabling` -> `enabled` after an enable, and the REMOVAL of the
//! record after a disable, since a pair that no longer propagates is not a propagation the
//! route table has at all. Both fall due on the same monotonic clock and at the same
//! [`TRANSITION_DELAY`] as every other move this store owes, and
//! [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] applies them.
//!
//! ## Capacity Blocks
//!
//! [`ComAmazonawsEc2Amazonec2State::capacity_blocks`] holds the account's Capacity Blocks
//! as [`CapacityBlockRecord`]s: the modelled [`CapacityBlock`] every operation that reports
//! one answers with, plus the state a writer pinned it to, keyed by `CapacityBlockId` in a
//! [`BTreeMap`] — the same stable order the VPC, subnet and volume stores are kept in, so a
//! pagination token can name the id the next page starts at. A block's `State` is NOT stored
//! on the view: "A Capacity Block expires" at its `EndDate` and its reserved window opens at
//! its `StartDate` (`CapacityBlock$EndDate` / `$StartDate`), so `scheduled` -> `active` ->
//! `expired` is a fact of where the WALL clock stands in that window and
//! [`CapacityBlockRecord::state_at`] derives it on read instead of letting two writers store
//! two different words for it. Only the states no clock drives — a block AWS could not
//! deliver, one that was cancelled, one whose payment is pending or failed — are stored, as
//! [`CapacityBlockRecord::pinned_state`].
//!
//! ## Secondary subnets
//!
//! [`ComAmazonawsEc2Amazonec2State::secondary_subnets`] holds the account's secondary
//! subnets — the subnets of a secondary network — as the modelled [`SecondarySubnet`]
//! shape itself, keyed by `SecondarySubnetId` in a [`BTreeMap`], exactly the way
//! [`ComAmazonawsEc2Amazonec2State::subnets`] holds ordinary subnets: a member a writer
//! sets round-trips through `DescribeSecondarySubnets` without a field-by-field copy, and
//! the id order is both the order the describe reports in and the position a pagination
//! token names. [`ComAmazonawsEc2Amazonec2State::insert_secondary_subnet`] is the one
//! place a record is built, so the id, the ARN, the owning account and the id of each
//! IPv4 CIDR block association are decided once however many writers there are.
//!
//! ## A Capacity Block OFFERING is a search result, not a stored resource
//!
//! Nothing in the EC2 API creates or deletes a Capacity Block offering:
//! `DescribeCapacityBlockOfferings` "Describes Capacity Block offerings available for
//! purchase in the Amazon Web Services Region that you're currently using", and the search
//! it answers is the reservation duration, instance count and date range the request names.
//! So an offering is derived — from the Region's [`availability_zones_in_region`], the
//! modelled instance-type catalog [`offered_instance_types`] and the wall clock — and
//! NOTHING about one is stored: two searches with the same terms in the same hour report the
//! same offerings, and the search writes nothing a later caller could see.
//!
//! The terms an offering was reported with therefore travel inside its id
//! ([`CapacityBlockOfferingTerms`], [`encode_capacity_block_offering_id`]) rather than in a
//! map keyed by it, which is what lets a purchase resolve the id a caller hands back —
//! window, zone and counts — without the describe having had to write a row for every
//! offering it merely reported. [`capacity_block_upfront_fee`] is the one definition of what
//! such a window costs.
//!
//! ## Application status is DERIVED, never stored
//!
//! [`ComAmazonawsEc2Amazonec2State::application_status`] holds, per `InstanceId`, the
//! per-check results an instance's application status checks reported and the suppression
//! in force for it — not the instance-level status itself, which
//! `ApplicationStatus$Status` defines as a function of those results
//! ([`aggregated_application_status`]). So an instance nothing has reported a check for is
//! `not-applicable`, the value the model documents for "no checks with `Aggregation` set
//! to `included` apply to the instance", and no two operations can disagree about one
//! instance's status.
//!
//! ## VM import tasks
//!
//! [`ComAmazonawsEc2Amazonec2State::import_image_tasks`] holds the import image tasks
//! `ImportImage` starts as [`ImportImageTaskRecord`]s: the modelled [`ImportImageTask`]
//! `DescribeImportImageTasks` reports, plus the instant the clock owes the task its
//! `active` -> `completed` move. Keyed by `ImportTaskId` in a [`BTreeMap`], the same
//! stable order the other stores are kept in, so a pagination token can name the id the
//! next page starts at. A task AWS hands back is `active`, so the completion — the AMI the
//! import creates, and the snapshot id of each imported disk — is owed by the clock and
//! applied once by
//! [`ComAmazonawsEc2Amazonec2State::complete_due_import_image_tasks`]; only the PERCENTAGE
//! of progress an active task reports is derived per read, being a fact of elapsed time
//! that no store could hold.
//!
//! ## Placement groups
//!
//! [`ComAmazonawsEc2Amazonec2State::placement_groups`] holds the account's placement
//! groups as the modelled [`PlacementGroup`] shape itself, keyed by `GroupId`, for the
//! reason [`ComAmazonawsEc2Amazonec2State::vpcs`] holds VPCs that way: `CreatePlacementGroup`
//! and `DescribePlacementGroups` answer the same shape, so storing it is what stops them
//! reporting one group differently. A placement group is "a logical grouping of instances"
//! (`CreatePlacementGroup`) with nothing to provision, so it is stored `available` and no
//! clock-owed move is recorded for it.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::Ipv6Addr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    Address, ApplicationStatus, ApplicationStatusDetail, AttachVolumeOutput, AvailabilityZone,
    AvailabilityZoneMessage, BlockDeviceMapping, BlockPublicAccessStates, ByoipCidr,
    CapacityBlock, ExportTask, FpgaImage, FpgaImageState, ImageUsageReport, LoadPermission,
    StorageLocation, VolumeRecycleBinInfo,
    CreateVolumeOutput, DescribeFastLaunchImagesSuccessItem, DisableFastLaunchOutput,
    VolumeModification,
    EbsBlockDevice, EnableFastLaunchOutput, GroupIdentifier, Host, INSTANCE_TYPE_VALUES, Image,
    ImageCriterion, Instance, InstanceApplicationStatus, IpamPool,
    InstanceState, InstanceTypeOffering, IpPermission, IpRange, LOCATION_TYPE_VALUES,
    LaunchTemplate, MacHost, Monitoring, OperatorResponse, PeeringAttachmentStatus, Placement,
    PlacementGroup,
    PrivateDnsNameOptionsOnLaunch,
    DnsEntry, PayerResponsibilityEntry,
    RequestLaunchTemplateData, RunInstancesOutput, SecondaryNetwork,
    SecondaryNetworkIpv4CidrBlockAssociation, SecondarySubnet,
    SecondarySubnetIpv4CidrBlockAssociation, SecurityGroup, ServiceConfiguration,
    SpotPlacementScore, StateReason,
    Subnet, SubnetAssociation, Tag,
    TagSpecification,
    TransitGateway, TransitGatewayClientVpnAttachment,
    TransitGatewayMulticastDomainAssociation, TransitGatewayOptions,
    TransitGatewayPeeringAttachment, TransitGatewayPropagation, TransitGatewayRequestOptions,
    TransitGatewayRouteTable, TransitGatewayRouteTableAnnouncement,
    TransitGatewayRouteTablePropagation,
    TransitGatewayVpcAttachment,
    UserIdGroupPair, Vpc, VpcBlockPublicAccessExclusion, VpcCidrBlockAssociation,
    VpcCidrBlockState, VpcEndpoint,
    VpcEndpointConnection,
};
use crate::scaffolded::Scaffolded;

/// The `Reservation` shape, under the name the scaffold generated for it.
///
/// `com.amazonaws.ec2#Reservation` is both `RunInstances`'s output shape and the
/// member type of `DescribeInstances`'s `Reservations` list, so the generator
/// emitted it once under the output name. This alias lets the code read as the
/// model does.
pub type ReservationView = RunInstancesOutput;

/// The one region whose Amazon-provided private DNS suffix is `ec2.internal`
/// rather than `<region>.compute.internal`.
pub const EC2_INTERNAL_REGION: &str = "us-east-1";

/// The Region a request belongs to when it carries no SigV4 credential scope to read
/// one from — `::engine::emulator_arn`'s documented default Region.
pub const DEFAULT_REGION: &str = EC2_INTERNAL_REGION;

/// The instance type `RunInstances` launches when the request names none.
///
/// AWS documents `m1.small` as `RunInstances`'s default instance type.
pub const DEFAULT_INSTANCE_TYPE: &str = "m1.small";

/// The maximum number of tags one EC2 resource may carry.
///
/// From the `CreateTags` documentation: "Each resource can have a maximum of 50
/// tags."
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The maximum number of resource or instance ids one request may name.
///
/// Documented on `CreateTags$Resources` ("Up to 1000 resource IDs") and on
/// `TerminateInstances$InstanceIds` ("Up to 1000 instance IDs"); the model declares
/// no `@length`, so the handler is the only place the bound can exist.
pub const MAX_RESOURCE_IDS: usize = 1000;

/// The most instances one `RunInstances` call may launch here.
///
/// The real service bounds `MaxCount` by the account's instance-type quota and
/// answers `InstanceLimitExceeded` above it. This emulator has no quota service, so
/// the ceiling is a fixed CHOSEN one. It is what keeps a single request's work and
/// allocation bounded: `MaxCount` arrives as a plain `i32` with no modelled
/// `@range`, so without this a request could ask for `i32::MAX` records.
pub const MAX_INSTANCES_PER_RUN: i32 = 64;

/// The most instances this emulator holds at once.
///
/// Also CHOSEN. Terminated instances are kept (the real service reports them for a
/// while), so without a ceiling repeated launches would grow memory without bound.
/// At most one reservation and one client token exist per launch, and every launch
/// mints at least one instance, so this bounds those two maps too.
pub const MAX_STORED_INSTANCES: usize = 4096;

/// The most filters one `DescribeInstances` call may carry.
///
/// CHOSEN, for the same reason: matching cost is the product of filters, values and
/// stored instances, and the model bounds none of the three.
pub const MAX_FILTERS: usize = 32;

/// The most values one `DescribeInstances` filter may carry. CHOSEN, see
/// [`MAX_FILTERS`].
pub const MAX_FILTER_VALUES: usize = 64;

/// The largest page `DescribeInstances` will serve, and the page size it uses when
/// the request names none.
///
/// AWS documents `MaxResults` for this operation with a maximum of 1000. Applying it
/// as the DEFAULT too is what stops a parameterless describe from materialising the
/// whole store in one response.
pub const MAX_PAGE_SIZE: usize = 1000;

/// The most matcher steps one `DescribeInstances` may spend on filtering.
///
/// CHOSEN, and it is the bound the per-factor caps cannot give: capping instances,
/// filters, values and value length each on their own still leaves their PRODUCT
/// free. The describe counts the comparisons it makes against this ceiling, which is
/// what keeps one request's filtering — and therefore the time it holds the state
/// lock — bounded however the caller combines the factors.
pub const MAX_FILTER_MATCH_STEPS: u64 = 2_000_000;

/// A countdown of matcher steps shared by every comparison one request makes.
///
/// Spending is saturating and the budget never refills, so once it is gone every
/// further comparison answers "no match" immediately and the caller sees
/// [`Self::exhausted`] — which is a different answer from "nothing matched" and is
/// reported as such.
#[derive(Debug)]
pub struct MatchBudget {
    remaining: u64,
}

impl MatchBudget {
    /// A budget of `steps` matcher steps.
    pub fn new(steps: u64) -> Self {
        Self { remaining: steps }
    }

    /// A budget nothing can exhaust, for a comparison made once.
    pub fn unlimited() -> Self {
        Self { remaining: u64::MAX }
    }

    /// Charge one step; `false` once the budget is gone.
    pub fn spend(&mut self) -> bool {
        if self.remaining == 0 {
            return false;
        }
        self.remaining -= 1;
        true
    }

    /// Whether the budget ran out.
    pub fn exhausted(&self) -> bool {
        self.remaining == 0
    }
}

/// The most bytes one `DescribeInstances` filter value may carry.
///
/// A capped NUMBER of comparisons is not a capped amount of work: the matcher's
/// cost is the product of the pattern's length and the value's, so the per-item
/// size needs a bound of its own. It is derived from [`MAX_TAG_VALUE_CHARS`] — the
/// longest value this store can hold — times UTF-8's maximum bytes per character,
/// so every stored value stays matchable by an exact filter.
pub const MAX_FILTER_VALUE_BYTES: usize = MAX_TAG_VALUE_CHARS * 4;

/// The most user data one launch may carry.
///
/// AWS documents a 16 KB limit on instance user data; the model declares no
/// `@length` on `RunInstancesUserData`, so the handler is the only place it can be
/// enforced. It is a BYTE bound, which a count bound cannot give: one request's
/// user data is otherwise cloned into every instance the request mints.
pub const MAX_USER_DATA_BYTES: usize = 16 * 1024;

/// The most security groups one instance may be launched with.
///
/// AWS documents a maximum of 16 security groups per network interface; the model
/// declares no `@length` on `SecurityGroupIds`/`SecurityGroups`, and the list is
/// cloned into every instance the request mints.
pub const MAX_SECURITY_GROUPS_PER_INSTANCE: usize = 16;

/// The most characters a tag key may carry. From `Tag$Key`'s own doc: "Tag keys are
/// case-sensitive and accept a maximum of 127 Unicode characters."
pub const MAX_TAG_KEY_CHARS: usize = 127;

/// The most characters a tag value may carry. From `Tag$Value`'s own doc: "Tag
/// values are case-sensitive and accept a maximum of 256 Unicode characters."
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// The most caller-chosen BYTES this emulator holds across every stored instance:
/// user data, client tokens, tags and security-group names.
///
/// CHOSEN, and it is the companion [`MAX_STORED_INSTANCES`] needs: a count-only
/// ceiling admits gigabytes, because each of those records holds strings the caller
/// chose the length of.
pub const MAX_STORED_BYTES: usize = 64 * 1024 * 1024;

/// How many candidate addresses [`ComAmazonawsEc2Amazonec2State::allocate_private_ip`]
/// may probe before giving up, so the search cannot spin.
const MAX_PRIVATE_IP_PROBES: u32 = 1 << 16;

/// How long after the call that starts it a clock-owed instance transition falls
/// due (`pending` -> `running`, `stopping` -> `stopped`, `shutting-down` ->
/// `terminated`).
///
/// CHOSEN, not an AWS-documented duration: the real service takes tens of
/// seconds. It is deliberately NON-ZERO, so a caller that reads straight after
/// the call still observes the transient state and the state errors that go with
/// it, and short enough that a test can wait it out. The due instant is stored;
/// nothing sleeps for it.
pub const TRANSITION_DELAY: Duration = Duration::from_millis(500);

/// The `DescribeInstances` filter names this slice resolves.
///
/// Source: the `Filters` member documentation of
/// `com.amazonaws.ec2#DescribeInstancesRequest`
/// (`smithy_query(query="doc",shape="DescribeInstancesRequest",member="Filters")`).
/// It lists many more; the ones here are those whose value this slice actually
/// stores. `tag:<key>` is matched separately because its name carries the key.
pub const SUPPORTED_INSTANCE_FILTERS: &[&str] = &[
    "architecture",
    "availability-zone",
    "client-token",
    "ebs-optimized",
    "image-id",
    "instance-id",
    "instance-state-code",
    "instance-state-name",
    "instance-type",
    "instance.group-id",
    "instance.group-name",
    "key-name",
    "launch-index",
    "monitoring-state",
    "owner-id",
    "private-dns-name",
    "private-ip-address",
    "reservation-id",
    "subnet-id",
    "tag-key",
];

/// The prefix of the `tag:<key>` filter family.
pub const TAG_FILTER_PREFIX: &str = "tag:";

/// Tag keys starting with this prefix are reserved for AWS and cannot be set
/// through `CreateTags`.
pub const RESERVED_TAG_PREFIX: &str = "aws:";

/// The resource types `RunInstances`'s `TagSpecifications` accepts.
///
/// From the member's documentation: "You can specify tags for the following
/// resources only: Instances Volumes Spot Instance requests Network interfaces".
/// Only the `instance` entry has a resource this slice stores; the others are
/// accepted and carry no instance tags.
pub const RUN_INSTANCES_TAGGABLE_RESOURCES: &[&str] = &[
    "instance",
    "volume",
    "spot-instances-request",
    "network-interface",
];

/// One of the six states `com.amazonaws.ec2#InstanceStateName` models.
///
/// The numeric codes are the ones the `instance-state-code` filter documents:
/// "0 (pending), 16 (running), 32 (shutting-down), 48 (terminated), 64
/// (stopping), and 80 (stopped)".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InstanceStateName {
    #[default]
    Pending,
    Running,
    ShuttingDown,
    Terminated,
    Stopping,
    Stopped,
}

impl InstanceStateName {
    /// The wire name of this state.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::ShuttingDown => "shutting-down",
            Self::Terminated => "terminated",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
        }
    }

    /// The 16-bit state code EC2 reports beside the name.
    pub fn code(self) -> i32 {
        match self {
            Self::Pending => 0,
            Self::Running => 16,
            Self::ShuttingDown => 32,
            Self::Terminated => 48,
            Self::Stopping => 64,
            Self::Stopped => 80,
        }
    }

    /// The modelled `InstanceState` for this state.
    pub fn view(self) -> InstanceState {
        InstanceState {
            code: Some(self.code()),
            name: Some(self.as_str().to_string()),
        }
    }

    /// Whether the instance is on its way to, or already in, `terminated` — the
    /// one end state an instance never leaves.
    pub fn is_terminating(self) -> bool {
        matches!(self, Self::ShuttingDown | Self::Terminated)
    }
}

/// A state change the CLOCK owes an instance: the state it moves to, and the
/// instant from which that move is due.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    pub to: InstanceStateName,
    pub due_at: Instant,
}

impl PendingTransition {
    /// A transition to `to`, due [`TRANSITION_DELAY`] from now.
    pub fn starting_now(to: InstanceStateName) -> Self {
        Self {
            to,
            due_at: Instant::now() + TRANSITION_DELAY,
        }
    }
}

/// One stored EC2 instance.
#[derive(Debug, Clone)]
pub struct InstanceRecord {
    pub instance_id: String,
    pub reservation_id: String,
    pub image_id: Option<String>,
    pub instance_type: String,
    pub key_name: Option<String>,
    pub subnet_id: Option<String>,
    /// The security groups the launch named, as `(group id, group name)`: a
    /// request may name either.
    pub security_groups: Vec<(Option<String>, Option<String>)>,
    pub private_ip: String,
    pub private_dns_name: String,
    pub public_ip: Option<String>,
    pub state: InstanceStateName,
    /// The next state the clock owes this instance, if any.
    pub pending_transition: Option<PendingTransition>,
    pub state_transition_reason: Option<String>,
    /// The `StateReason` code and message, set when the caller moved the state.
    pub state_reason: Option<(String, String)>,
    pub launch_time: SystemTime,
    pub ami_launch_index: i32,
    /// Stored because the launch supplied it; `DescribeInstances` never reports user
    /// data (the real service returns it from `DescribeInstanceAttribute`, which is
    /// still a stub).
    pub user_data: Option<String>,
    pub placement_az: String,
    pub monitoring_state: String,
    pub architecture: String,
    pub ebs_optimized: bool,
    pub hibernation_configured: bool,
    pub disable_api_termination: bool,
    pub tags: BTreeMap<String, String>,
    pub client_token: Option<String>,
}

/// One stored reservation: the unit of state one `RunInstances` call creates.
#[derive(Debug, Clone, Default)]
pub struct ReservationRecord {
    pub reservation_id: String,
    pub owner_id: String,
    pub requester_id: Option<String>,
    /// Its instances, in launch order (so index == `ami_launch_index`).
    pub instance_ids: Vec<String>,
}

/// How many Availability Zones a Region reports.
///
/// AWS gives a standard Region at least three Availability Zones; the exact count
/// varies by Region and the zone-name -> zone-id mapping varies by account, neither of
/// which a request carries, so every Region here reports exactly that guaranteed
/// minimum with a stable name -> id mapping.
pub const AVAILABILITY_ZONES_PER_REGION: usize = 3;

/// `AvailabilityZone$ZoneType` for an Availability Zone, as opposed to a Local Zone or
/// a Wavelength Zone. From the member's own doc ("Valid values:
/// `availability-zone` | `local-zone` | `wavelength-zone`"), via
/// `smithy_query(query="shape",shape="AvailabilityZone")`.
pub const ZONE_TYPE_AVAILABILITY_ZONE: &str = "availability-zone";

/// `AvailabilityZone$State` for a zone that is serving normally — one of the
/// `AvailabilityZoneState` enum's values
/// (`smithy_query(query="shape",shape="AvailabilityZoneState")`).
pub const ZONE_STATE_AVAILABLE: &str = "available";

/// `AvailabilityZone$OptInStatus` for an Availability Zone: the member's doc says
/// "For Availability Zones, this parameter always has the value of
/// `opt-in-not-required`".
pub const OPT_IN_NOT_REQUIRED: &str = "opt-in-not-required";

/// EC2's error code for a request member carrying a value the service cannot accept.
/// EC2 models no error shapes at all (`smithy_query(query="operation", …)` reports
/// `errors: []`), so this code is answered through
/// [`crate::scaffolded::Scaffolded::service_error`].
pub const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// EC2's error code for a request that leaves out a member the action requires.
pub const MISSING_PARAMETER: &str = "MissingParameter";

/// EC2's error code for members that are each valid but cannot be used together.
pub const INVALID_PARAMETER_COMBINATION: &str = "InvalidParameterCombination";

/// EC2's error code for a `DryRun` request whose caller holds the permissions the
/// action needs: "If you have the required permissions, the error response is
/// `DryRunOperation`" (the `DryRun` member's own doc).
pub const DRY_RUN_OPERATION: &str = "DryRunOperation";

/// EC2's error code for a request that named a VPC id no VPC of the account has.
pub const INVALID_VPC_ID_NOT_FOUND: &str = "InvalidVpcID.NotFound";

/// EC2's error code for a request that named a subnet id no subnet of the account has.
pub const INVALID_SUBNET_ID_NOT_FOUND: &str = "InvalidSubnetID.NotFound";

/// EC2's error code for a request that named a security group no group of the account has.
pub const INVALID_GROUP_NOT_FOUND: &str = "InvalidGroup.NotFound";

/// EC2's error code for a request that named an address-pool id no pool of the account
/// has — the code the customer-owned-IP surface answers with (`InvalidPoolID.NotFound`,
/// as recorded for a `CoipPoolId` that names no pool).
pub const INVALID_POOL_ID_NOT_FOUND: &str = "InvalidPoolID.NotFound";

/// EC2's error code for a pagination token this service did not issue.
pub const INVALID_PAGINATION_TOKEN: &str = "InvalidPaginationToken";

/// EC2's error code for a request that named a volume id this account holds no EBS
/// volume for.
pub const INVALID_VOLUME_ID_NOT_FOUND: &str = "InvalidVolume.NotFound";

/// EC2's error code for a volume id that is not shaped like one, answered BEFORE the
/// id is looked up — the same ordering `require_known_instance` keeps for instances.
pub const INVALID_VOLUME_ID_MALFORMED: &str = "InvalidVolumeID.Malformed";

/// EC2's error code for a request that named a snapshot id this account holds no
/// snapshot for.
pub const INVALID_SNAPSHOT_ID_NOT_FOUND: &str = "InvalidSnapshot.NotFound";

/// EC2's error code for a snapshot id that is not shaped like one, answered BEFORE the
/// id is looked up — the same ordering `require_known_volume` keeps for a volume id.
pub const INVALID_SNAPSHOT_ID_MALFORMED: &str = "InvalidSnapshotID.Malformed";

/// EC2's error code for a request that would take the account past its EBS volume
/// quota. A ceiling on what is STORED is a quota, not something the request got wrong,
/// so it is answered with the quota code an SDK classifies as one rather than with
/// `InvalidParameterValue`.
pub const VOLUME_LIMIT_EXCEEDED: &str = "VolumeLimitExceeded";

/// EC2's error code for a request that named an EBS volume this account holds no
/// modification for: the volume resolves, so it is the MODIFICATION that is missing, and
/// an absent modification is an error rather than an empty answer.
pub const INVALID_VOLUME_MODIFICATION_NOT_FOUND: &str = "InvalidVolumeModification.NotFound";

/// EC2's error code for a failure of the emulator itself, never for anything a request
/// carries: a 5xx is retryable to an SDK's retry classifier.
pub const INTERNAL_ERROR: &str = "InternalError";

/// EC2's error code for a request that named a launch template id this account holds no
/// launch template for.
pub const INVALID_LAUNCH_TEMPLATE_ID_NOT_FOUND: &str = "InvalidLaunchTemplateId.NotFound";

/// EC2's error code for a request that named something that is not a launch template id
/// at all, so there is nothing to look up.
pub const INVALID_LAUNCH_TEMPLATE_ID_MALFORMED: &str = "InvalidLaunchTemplateId.Malformed";

/// EC2's error code for a request that named a launch template name no launch template of
/// this account carries.
pub const INVALID_LAUNCH_TEMPLATE_NAME_NOT_FOUND: &str =
    "InvalidLaunchTemplateName.NotFoundException";

/// EC2's error code for a create that named a launch template name this account already
/// holds a launch template under: `DeleteLaunchTemplate` and `DescribeLaunchTemplates`
/// resolve a template by NAME as well as by id
/// (`smithy_query(query="operation",operation="DeleteLaunchTemplate")`), so a name can
/// only ever name one template.
pub const INVALID_LAUNCH_TEMPLATE_NAME_ALREADY_EXISTS: &str =
    "InvalidLaunchTemplateName.AlreadyExistsException";

/// EC2's error code for a create that would take the account past the number of launch
/// templates it may hold.
pub const LAUNCH_TEMPLATE_LIMIT_EXCEEDED: &str = "LaunchTemplateLimitExceeded";

/// The most launch templates this emulator holds at once.
///
/// The real service bounds them with the account's "Launch templates per Region" quota of
/// 5,000 (Amazon EC2 service quotas); this emulator has no quota service, so the same
/// number is the fixed ceiling, and it is what keeps the store's growth bounded however
/// many times a caller creates one.
pub const MAX_LAUNCH_TEMPLATES: usize = 5_000;

/// The fewest characters a launch template name may carry: the `@length` minimum of the
/// modelled `LaunchTemplateName`
/// (`smithy_query(query="shape",shape="LaunchTemplateName")`), which is the type every
/// operation that REPORTS or NAMES a template uses.
pub const MIN_LAUNCH_TEMPLATE_NAME_CHARS: usize = 3;

/// The most characters a launch template name may carry: that same `@length` maximum.
pub const MAX_LAUNCH_TEMPLATE_NAME_CHARS: usize = 128;

/// Every character the modelled `LaunchTemplateName` pattern
/// `^[a-zA-Z0-9\(\)\.\-/_]+$` admits beside the ASCII letters and digits.
pub const LAUNCH_TEMPLATE_NAME_PUNCTUATION: &str = "()./-_";

/// The most characters a `ClientToken` may carry. From `CreateLaunchTemplate$ClientToken`'s
/// own doc: "Constraint: Maximum 128 ASCII characters."
pub const MAX_CLIENT_TOKEN_CHARS: usize = 128;

/// The resource type `CreateLaunchTemplate$TagSpecifications` accepts.
///
/// From the member's documentation: "To tag the launch template, the resource type must be
/// `launch-template`." Any other type names a resource this operation does not create.
pub const LAUNCH_TEMPLATE_TAGGABLE_RESOURCE: &str = "launch-template";

/// The version number of the first version of a launch template: `CreateLaunchTemplate`
/// creates "the first version of the launch template"
/// (`CreateLaunchTemplate$VersionDescription`), and `LaunchTemplateVersion$VersionNumber`
/// counts from one.
pub const FIRST_LAUNCH_TEMPLATE_VERSION: i64 = 1;

/// Every filter `DescribeAvailabilityZones` documents, as its `Filters` member lists
/// them
/// (`smithy_query(query="doc",shape="DescribeAvailabilityZonesRequest",member="Filters")`).
/// A filter outside this set is refused; a filter inside it is answered by
/// [`AvailabilityZoneRecord::filter_values`], which must cover every entry.
pub const AVAILABILITY_ZONE_FILTERS: [&str; 11] = [
    "group-long-name",
    "group-name",
    "message",
    "opt-in-status",
    "parent-zone-id",
    "parent-zone-name",
    "region-name",
    "state",
    "zone-id",
    "zone-name",
    "zone-type",
];

/// The IPv4 CIDR block of an account's default VPC.
///
/// AWS creates one default VPC per Region for every account (every account created after
/// 2013-12-04 in a Region that supports them), with "an IPv4 CIDR block of
/// `172.31.0.0/16`" — Amazon VPC User Guide, "Default VPCs". A fresh account therefore
/// already has a VPC before anything calls `CreateVpc`, which is why the VPC store starts
/// with one rather than empty.
pub const DEFAULT_VPC_CIDR_BLOCK: &str = "172.31.0.0/16";

/// The tenancy of instances launched into a VPC created without an explicit one: the
/// `Tenancy` enum's `default` member.
pub const DEFAULT_INSTANCE_TENANCY: &str = "default";

/// A VPC AWS has finished creating: the `VpcState` enum's `available` member.
pub const VPC_STATE_AVAILABLE: &str = "available";

/// An IPv4 CIDR block associated with its VPC: the `VpcCidrBlockStateCode` enum's
/// `associated` member.
pub const CIDR_BLOCK_STATE_ASSOCIATED: &str = "associated";

/// A subnet AWS has finished creating: the `SubnetState` enum's `available` member
/// (`smithy_query(query="shape",shape="SubnetState")`).
pub const SUBNET_STATE_AVAILABLE: &str = "available";

/// How many IPv4 addresses of a subnet's CIDR block are NOT available to a network
/// interface.
///
/// "AWS reserves five IP addresses in each subnet CIDR block" — the network address, the
/// VPC router, the Amazon DNS address, one reserved for future use, and the broadcast
/// address (Amazon VPC User Guide, "Subnet CIDR blocks"). It is what makes
/// `Subnet$AvailableIpAddressCount` smaller than the block itself.
pub const SUBNET_RESERVED_ADDRESSES: u32 = 5;

/// The prefix length of each default subnet of a default VPC: "a size /20 IPv4 CIDR
/// block" (Amazon VPC User Guide, "Default VPCs"), carved out of the default VPC's
/// [`DEFAULT_VPC_CIDR_BLOCK`].
pub const DEFAULT_SUBNET_PREFIX_LENGTH: u32 = 20;

/// The hostname type a subnet assigns at launch unless something changes it: an instance
/// DNS name based on its IPv4 address, which is the `HostnameType` enum's `ip-name`
/// member and the default for an IPv4 subnet
/// (`PrivateDnsNameOptionsOnLaunch$HostnameType`).
pub const HOSTNAME_TYPE_IP_NAME: &str = "ip-name";

/// The smallest device position a subnet's local network interfaces may sit at.
///
/// "For example, 1 indicates local network interfaces in this subnet are the secondary
/// network interface (eth1). A local network interface cannot be the primary network
/// interface (eth0)" (`ModifySubnetAttributeRequest$EnableLniAtDeviceIndex`), so position 0
/// — and anything below it — names the primary interface and is refused.
pub const MIN_LNI_DEVICE_INDEX: i32 = 1;

/// The prefix of a customer-owned IPv4 address pool id (`CoipPoolId`).
pub const COIP_POOL_ID_PREFIX: &str = "ipv4pool-coip-";

/// Whether `id` is a well-formed customer-owned IPv4 address pool id.
///
/// A COIP pool id is [`COIP_POOL_ID_PREFIX`] followed by 8 or 17 lowercase hexadecimal
/// digits — the short and the long EC2 resource id forms, read the same way
/// [`allocation_id_is_well_formed`] reads an allocation id. `CoipPoolId` is a bare string in
/// the model, so this is also what BOUNDS the bytes a caller can park on a subnet through
/// `ModifySubnetAttribute`: the pool id a subnet retains is at most this long.
pub fn coip_pool_id_is_well_formed(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix(COIP_POOL_ID_PREFIX) else {
        return false;
    };
    matches!(suffix.len(), 8 | 17)
        && suffix
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// The name of the default security group every VPC comes with: "the default security
/// group is named `default`" (Amazon VPC User Guide, "Default security groups for your
/// VPCs"). It is also the one group name a caller may name a group by.
pub const DEFAULT_SECURITY_GROUP_NAME: &str = "default";

/// The description EC2 gives the default security group of a VPC.
pub const DEFAULT_SECURITY_GROUP_DESCRIPTION: &str = "default VPC security group";

/// Every IP protocol: "Use `-1` to specify all protocols" (`IpPermission$IpProtocol`),
/// which is what the default security group's own rules allow.
pub const ALL_IP_PROTOCOLS: &str = "-1";

/// Every IPv4 address, as a CIDR block — the destination of the default security group's
/// outbound rule.
pub const ALL_IPV4_ADDRESSES: &str = "0.0.0.0/0";

/// VPC Block Public Access turned off — "`off`: VPC BPA is not enabled and traffic is
/// allowed to and from internet gateways and egress-only internet gateways in this
/// Region" (`BlockPublicAccessStates$InternetGatewayBlockMode`), which is the state of an
/// account that has never enabled it.
pub const BLOCK_PUBLIC_ACCESS_OFF: &str = "off";

/// Per-service state for com-amazonaws-ec2-amazonec2. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsEc2Amazonec2State {
    /// Reservations by `ReservationId`.
    pub reservations: HashMap<String, ReservationRecord>,
    /// Reservation ids in launch order: the stable order `DescribeInstances`
    /// reports and pages over.
    pub reservation_order: Vec<String>,
    /// Instances by `InstanceId`.
    pub instances: HashMap<String, InstanceRecord>,
    /// A `RunInstances` `ClientToken` and the reservation it minted, so a repeat
    /// of the same request returns that reservation instead of launching again.
    pub client_tokens: HashMap<String, String>,
    /// Every primary private address in use, so allocating the next one is a
    /// constant-time check rather than a scan of every stored instance.
    assigned_private_ips: ::std::collections::HashSet<String>,
    /// The caller-chosen bytes the stored instances hold, bounded by
    /// [`MAX_STORED_BYTES`].
    stored_bytes: usize,
    next_instance: u64,
    next_reservation: u64,
    next_private_ip: u32,
    /// Every VPC the account holds, by `VpcId`.
    ///
    /// Availability Zones are NOT here: they are derived from the Region (see the
    /// module docs). Further caller-created resources go beside this field as they
    /// land.
    pub vpcs: BTreeMap<String, Vpc>,
    /// When each VPC's `pending` -> `available` transition falls due, for the VPCs that
    /// have one.
    ///
    /// `CreateVpc` answers a VPC in the non-terminal `pending` state, so the clock owes it
    /// `available` (and its IPv6 association `associated`) [`TRANSITION_DELAY`] later — the
    /// same span, read off the same monotonic clock, as an instance's
    /// [`PendingTransition`]. The due instant is recorded rather than slept for, and
    /// [`Self::materialize_due_transitions`] applies it once the clock has passed it; a VPC
    /// AWS returns ready (the account's default VPC) never has an entry here.
    pub vpc_available_at: BTreeMap<String, Instant>,
    /// The per-VPC attributes `DescribeVpcAttribute` reports and `ModifyVpcAttribute`
    /// sets, by `VpcId`.
    ///
    /// The modelled [`Vpc`] shape carries no member for any of them — no describe of a
    /// VPC reports them — so they live beside the VPC record keyed by the same id, the
    /// way [`Self::vpc_available_at`] does. Every VPC this store holds has an entry:
    /// [`Self::insert_vpc`] writes one at the values AWS gives a VPC on creation
    /// ([`VpcAttributes::for_vpc`]), so a describe never has to invent a default for a
    /// VPC nobody has modified.
    pub vpc_attributes: BTreeMap<String, VpcAttributes>,
    /// Every association between a local gateway route table and a virtual interface group
    /// the account holds, by `LocalGatewayRouteTableVirtualInterfaceGroupAssociationId`, as
    /// the modelled `LocalGatewayRouteTableVirtualInterfaceGroupAssociation` shape itself —
    /// so a member a writer sets round-trips through
    /// `DescribeLocalGatewayRouteTableVirtualInterfaceGroupAssociations` without a
    /// field-by-field copy, the way the VPC and subnet stores hold their shapes. Ordered by
    /// id, which is the order that describe reports and pages over.
    ///
    /// Starts EMPTY, unlike the stores above it: a local gateway and the route tables and
    /// virtual interface groups on it exist only on an Outpost, which no EC2 call creates,
    /// so an account without one has no association to report.
    /// [`Self::insert_local_gateway_route_table_virtual_interface_group_association`] is the
    /// one way an association is stored.
    pub local_gateway_route_table_virtual_interface_group_associations: BTreeMap<
        String,
        crate::ops::types::LocalGatewayRouteTableVirtualInterfaceGroupAssociation,
    >,
    /// Every subnet the account holds, by `SubnetId`, as the modelled [`Subnet`] shape
    /// itself — so a member a writer sets round-trips through `DescribeSubnets` without a
    /// field-by-field copy. Ordered by id, which is the order the describe reports and
    /// pages over.
    pub subnets: BTreeMap<String, Subnet>,
    /// When each subnet's `pending` -> `available` transition falls due, for the subnets
    /// that have one.
    ///
    /// The subnet counterpart of [`Self::vpc_available_at`], for the same reason and on the
    /// same clock: `CreateSubnet` answers a subnet in the non-terminal `pending` state, so
    /// the clock owes it `available` [`TRANSITION_DELAY`] later whether or not anybody
    /// calls again. A subnet AWS hands over ready — a default VPC's default subnets — never
    /// has an entry here.
    pub subnet_available_at: BTreeMap<String, Instant>,
    /// Every Verified Access trust provider the account holds, by
    /// `VerifiedAccessTrustProviderId`, as the modelled
    /// [`crate::ops::types::VerifiedAccessTrustProvider`] shape itself — so a member a
    /// writer sets round-trips through `DescribeVerifiedAccessTrustProviders` without a
    /// field-by-field copy. Ordered by id, which is the order that describe reports and
    /// pages over.
    pub verified_access_trust_providers:
        BTreeMap<String, crate::ops::types::VerifiedAccessTrustProvider>,
    /// A `CreateVerifiedAccessTrustProvider` `ClientToken` and the trust provider id it
    /// minted, so a repeat of the same request answers with that trust provider instead
    /// of creating a second one.
    pub verified_access_trust_provider_client_tokens: HashMap<String, String>,
    /// Every AMI the account can see, by `ImageId`, as the modelled [`Image`] shape itself
    /// — the Amazon-published catalog a fresh account already has (see the module docs)
    /// plus any image a writer stores later. Ordered by id, which is the order the
    /// describe reports and pages over.
    pub images: BTreeMap<String, Image>,
    /// Every security group the account holds, by `GroupId`, as the modelled
    /// [`SecurityGroup`] shape itself — so a member a writer sets round-trips through
    /// `DescribeSecurityGroups` without a field-by-field copy. Ordered by id, which is the
    /// order the describe reports and pages over.
    pub security_groups: BTreeMap<String, SecurityGroup>,
    /// Every launch template the account holds, by `LaunchTemplateId`, each with its
    /// versions — see the module docs.
    pub launch_templates: BTreeMap<String, LaunchTemplateRecord>,
    /// A `CreateLaunchTemplate` `ClientToken` and the launch template id it minted, so a
    /// repeat of the same request answers with that template instead of creating a second
    /// one. Cleared for a template a delete removes, so the token cannot name a template
    /// that is gone.
    pub launch_template_client_tokens: HashMap<String, String>,
    /// Monotonic source of generated resource-id suffixes, so an id is unique within a
    /// run and deterministic across runs.
    pub id_counter: u64,
    /// Every EBS volume the account holds, by `VolumeId` — see the module docs. Ordered
    /// by id, which is the order `DescribeVolumes` reports and pages over.
    pub volumes: BTreeMap<String, VolumeRecord>,
    /// A `CreateVolume` `ClientToken` and the volume id it minted, so a repeat of the
    /// same request answers with that volume instead of creating a second one.
    pub volume_client_tokens: HashMap<String, String>,
    /// The most recent modification of each EBS volume that has one, by `VolumeId` — see
    /// the module docs. Ordered by that id, which is the order
    /// `DescribeVolumesModifications` reports and pages over.
    ///
    /// Keyed by a volume of [`Self::volumes`], so this store can never hold more records
    /// than that one holds volumes ([`MAX_STORED_VOLUMES`]), and every string a record
    /// carries is either derived here or a value the modelled `VolumeType` enum admits —
    /// nothing caller-sized — so no byte charge is made for one.
    pub volume_modifications: BTreeMap<String, VolumeModificationRecord>,
    /// Every secondary network the account holds, by `SecondaryNetworkId` — see the module
    /// docs. Ordered by id, which is the order `DescribeSecondaryNetworks` reports and
    /// pages over.
    pub secondary_networks: BTreeMap<String, SecondaryNetworkRecord>,
    /// A `CreateSecondaryNetwork` `ClientToken` and the secondary network id it minted, so
    /// a repeat of the same request answers with that network instead of creating a second
    /// one.
    pub secondary_network_client_tokens: HashMap<String, String>,
    /// Every BYOIP address range the account holds, by the range itself in canonical form
    /// — see the module docs. Ordered by that key, which is the order a describe of them
    /// reports and pages over.
    pub byoip_cidrs: BTreeMap<String, ByoipCidrRecord>,
    /// Every customer-owned IP (CoIP) address pool the account holds, by `PoolId`, as the
    /// modelled `CoipPool` shape itself — so every member a writer sets round-trips through
    /// `DescribeCoipPools` without a field-by-field copy. Ordered by id in a [`BTreeMap`],
    /// the same arrangement the VPC and subnet stores use: that id order is the one stable
    /// order the describe reports and pages over, so a pagination token can name the id the
    /// next page starts at.
    ///
    /// Starts EMPTY: a CoIP pool exists only on an Outpost local gateway, so a fresh
    /// account has none until something creates one through [`Self::insert_coip_pool`].
    pub coip_pools: BTreeMap<String, crate::ops::types::CoipPool>,
    /// Every Capacity Block the account holds, by `CapacityBlockId` — see the module docs.
    /// Ordered by id, which is the order `DescribeCapacityBlocks` reports and pages over.
    pub capacity_blocks: BTreeMap<String, CapacityBlockRecord>,
    /// What each instance's application health status is derived from, by `InstanceId` —
    /// see [`InstanceApplicationStatusRecord`] and the module docs. An instance with no
    /// entry has had no application status check reported for it, which is exactly the
    /// `not-applicable` instance-level status `DescribeApplicationStatus` answers with.
    pub application_status: BTreeMap<String, InstanceApplicationStatusRecord>,
    /// The Windows fast launch configuration of every image that has one, by `ImageId` —
    /// see the module docs. Ordered by id, which is the order
    /// `DescribeFastLaunchImages` reports and pages over.
    pub fast_launch_images: BTreeMap<String, FastLaunchRecord>,
    /// Every Elastic IP address the account holds, by `AllocationId`, as the modelled
    /// [`Address`] shape itself — so a member a writer sets round-trips through
    /// `DescribeAddresses` without a field-by-field copy. Ordered by id, which is the
    /// order the describe reports in.
    pub addresses: BTreeMap<String, Address>,
    /// Monotonic source of the Elastic IP addresses [`Self::mint_elastic_ip`] hands out,
    /// so one allocation can never be given an address another still holds.
    next_elastic_ip: u64,
    /// Every transit gateway the account holds, by `TransitGatewayId`, as the modelled
    /// [`TransitGateway`] shape itself — so a member a writer sets round-trips through
    /// `DescribeTransitGateways` without a field-by-field copy. Ordered by id, which is
    /// the order the describe reports and pages over.
    pub transit_gateways: BTreeMap<String, TransitGateway>,
    /// When each transit gateway's `pending` -> `available` transition falls due, for the
    /// gateways that have one.
    ///
    /// The transit gateway counterpart of [`Self::vpc_available_at`], for the same reason
    /// and on the same clock: `CreateTransitGateway` answers a gateway in the non-terminal
    /// `pending` state ("After the transit gateway enters the available state, you can
    /// attach your VPCs and VPN connections to the transit gateway"), so the clock owes it
    /// `available` [`TRANSITION_DELAY`] later whether or not anybody calls again. The
    /// gateway's default route table moves with it.
    pub transit_gateway_available_at: BTreeMap<String, Instant>,
    /// Every transit gateway route table the account holds, by
    /// `TransitGatewayRouteTableId` — starting with the default one each
    /// `CreateTransitGateway` makes (see the module docs). Ordered by id, the stable order
    /// every reader of it reports in.
    pub transit_gateway_route_tables: BTreeMap<String, TransitGatewayRouteTable>,
    /// Which attachment propagates its routes into which transit gateway route table, by
    /// (`TransitGatewayRouteTableId`, `TransitGatewayAttachmentId`) — see
    /// [`TransitGatewayPropagationRecord`] and the module docs. One entry per pair, because
    /// a pair either propagates or does not; ordered by route table and then by attachment,
    /// which is the order `GetTransitGatewayRouteTablePropagations` reports and pages over.
    pub transit_gateway_propagations: BTreeMap<(String, String), TransitGatewayPropagationRecord>,
    /// Every Dedicated Host the account holds, by `HostId`, as the modelled [`Host`]
    /// shape itself — so a member a writer sets round-trips through a describe without a
    /// field-by-field copy that can silently drop one. Ordered by id, which is the order
    /// the describe operations report and page over.
    pub dedicated_hosts: BTreeMap<String, Host>,
    /// An `AllocateHosts` `ClientToken` and the host ids that request minted, so a repeat
    /// of the same request answers with those hosts instead of allocating more. One token
    /// names EVERY host of the one allocation, because one call allocates `Quantity` of
    /// them.
    pub dedicated_host_client_tokens: HashMap<String, Vec<String>>,
    /// Every connection to one of the account's own VPC endpoint services, by
    /// `VpcEndpointId`, as the modelled [`VpcEndpointConnection`] shape itself — see the
    /// module docs. Ordered by endpoint id, which is the order
    /// `DescribeVpcEndpointConnections` reports and pages over.
    pub vpc_endpoint_connections: BTreeMap<String, VpcEndpointConnection>,
    /// The account's Allowed AMIs setting in each Region a caller has configured it in, by
    /// Region name — see the module docs. A Region with no entry has the setting an account
    /// that never enabled the feature has ([`AllowedImagesSettings::default`]), so a read
    /// never has to store one.
    pub allowed_images_settings: BTreeMap<String, AllowedImagesSettings>,
    /// Every Amazon FPGA Image (AFI) the account holds, by `FpgaImageId` — see
    /// [`FpgaImageRecord`]. Ordered by id, which is the order a report of them pages over.
    pub fpga_images: BTreeMap<String, FpgaImageRecord>,
    /// A `CreateFpgaImage` `ClientToken` and the AFI id it minted, so a repeat of the same
    /// request answers with that AFI instead of creating a second one. Cleared for an AFI a
    /// delete removes, so a token cannot name an image that is gone.
    pub fpga_image_client_tokens: HashMap<String, String>,
    /// Every volume in the account's Recycle Bin, by `VolumeId`, as the modelled
    /// [`VolumeRecycleBinInfo`] shape `ListVolumesInRecycleBin` reports — so a member a
    /// writer sets round-trips without a field-by-field copy. Ordered by id, which is the
    /// order that report pages over.
    ///
    /// A volume reaches the bin when it is deleted while a Recycle Bin retention rule
    /// covers it, and leaves it when it is restored or when its retention period runs out;
    /// `DeleteVolume` and `RestoreVolumeFromRecycleBin` are the operations that move it,
    /// and both are still scaffolds, so a fresh account's bin is empty — which is exactly
    /// what the report answers with.
    pub volumes_in_recycle_bin: BTreeMap<String, VolumeRecycleBinInfo>,
    /// Every AMI usage report the account holds, by `ReportId`, as the modelled
    /// [`ImageUsageReport`] shape the describe reports. `CreateImageUsageReport` is the
    /// operation that adds one and `DeleteImageUsageReport` the one that removes it.
    pub image_usage_reports: BTreeMap<String, ImageUsageReport>,
    /// Every instance export task the account holds, by `ExportTaskId` — see
    /// [`ExportTaskRecord`]. `CreateInstanceExportTask` is the operation that starts one and
    /// `CancelExportTask` the one that cancels it.
    pub export_tasks: BTreeMap<String, ExportTaskRecord>,
    /// Every secondary subnet the account holds, by `SecondarySubnetId`, as the modelled
    /// [`SecondarySubnet`] shape itself — see the module docs. Ordered by id, which is the
    /// order `DescribeSecondarySubnets` reports and pages over.
    pub secondary_subnets: BTreeMap<String, SecondarySubnet>,
    /// The state move an acceptance owes a VPC endpoint connection, by `VpcEndpointId` —
    /// the connection counterpart of [`Self::vpc_available_at`], beside the connection
    /// store rather than inside it because that store holds the modelled shape itself.
    /// `AcceptVpcEndpointConnections` leaves the connection in the non-terminal `Pending`
    /// state, so the clock owes it `Available` [`TRANSITION_DELAY`] later; a connection
    /// nothing has accepted has no entry here.
    pub vpc_endpoint_connection_moves: BTreeMap<String, PendingStateMove>,
    /// Every VPC endpoint service configuration the account owns, by `ServiceId`, as the
    /// modelled [`ServiceConfiguration`] shape itself. This is the set a `ServiceId` a
    /// request names is resolved against, so an id naming no service of the account is
    /// refused rather than read as "no connections".
    pub vpc_endpoint_services: BTreeMap<String, ServiceConfiguration>,
    /// Every VPC endpoint the account holds, by `VpcEndpointId`, as the modelled
    /// [`VpcEndpoint`] shape itself. Ordered by id, which is the order
    /// `DescribeVpcEndpoints` reports and pages over.
    pub vpc_endpoints: BTreeMap<String, VpcEndpoint>,
    /// Every transit gateway attachment the account holds, by
    /// `TransitGatewayAttachmentId` — ONE map for every kind of attachment, because one id
    /// space serves them all and an id therefore identifies at most one attachment of one
    /// kind. See [`TransitGatewayAttachmentRecord`].
    pub transit_gateway_attachments: BTreeMap<String, TransitGatewayAttachmentRecord>,
    /// Every transit gateway multicast domain the account holds, by
    /// `TransitGatewayMulticastDomainId`, each with its subnet associations.
    pub transit_gateway_multicast_domains: BTreeMap<String, TransitGatewayMulticastDomainRecord>,
    /// Every transit gateway route table announcement the account holds, by
    /// `TransitGatewayRouteTableAnnouncementId`.
    pub transit_gateway_route_table_announcements:
        BTreeMap<String, TransitGatewayRouteTableAnnouncementRecord>,
    /// Every IPAM pool the account holds, by `IpamPoolId`, as the modelled [`IpamPool`]
    /// shape itself.
    pub ipam_pools: BTreeMap<String, IpamPool>,
    /// Every VPC Block Public Access exclusion the account holds, by `ExclusionId` — see
    /// the module docs. Ordered by id, which is the order
    /// `DescribeVpcBlockPublicAccessExclusions` reports and pages over.
    pub vpc_block_public_access_exclusions: BTreeMap<String, VpcBlockPublicAccessExclusionRecord>,
    /// Every import image task the account has started, by `ImportTaskId` — see the module
    /// docs. Ordered by id, which is the order `DescribeImportImageTasks` reports and pages
    /// over.
    pub import_image_tasks: BTreeMap<String, ImportImageTaskRecord>,
    /// An `ImportImage` `ClientToken` and the import task id it minted, so a repeat of the
    /// same request answers with that task instead of starting a second import: "The token
    /// to enable idempotency for VM import requests" (`ImportImageRequest$ClientToken`).
    pub import_image_task_client_tokens: HashMap<String, String>,
    /// The account's placement groups, as the modelled [`PlacementGroup`] shape every
    /// operation that reports one answers with, keyed by `GroupId` in a [`BTreeMap`] — the
    /// same stable order the other stores are kept in, which is the order
    /// `DescribePlacementGroups` reports them in. A placement group is a Region-scoped
    /// logical grouping with no network of its own, so nothing is stored beside the
    /// modelled shape: every fact a reader needs ("the name", "the state", "the strategy",
    /// "the number of partitions", "the spread level", "the ARN", "any tags applied to the
    /// placement group" — [`PlacementGroup`]) is a member of it.
    pub placement_groups: BTreeMap<String, PlacementGroup>,
    /// The state of the account's *block public access for snapshots* setting — see the
    /// module docs.
    ///
    /// A `&'static str` so only a member of the modelled `SnapshotBlockPublicAccessState`
    /// catalog can ever be stored here: the word comes from this module's own constants,
    /// never from a request. Read it through
    /// [`Self::snapshot_block_public_access_state`] and write it through
    /// [`Self::set_snapshot_block_public_access_state`], so the operations that report and
    /// the operations that change it cannot disagree.
    snapshot_block_public_access: &'static str,
}

/// A fresh account's EC2 state: the ambient network AWS gives it — the default VPC, that
/// VPC's per-zone default subnets and its default security group — plus the public images
/// Amazon publishes, and nothing else.
impl Default for ComAmazonawsEc2Amazonec2State {
    fn default() -> Self {
        let mut state = Self {
            reservations: HashMap::new(),
            reservation_order: Vec::new(),
            instances: HashMap::new(),
            client_tokens: HashMap::new(),
            assigned_private_ips: ::std::collections::HashSet::new(),
            stored_bytes: 0,
            next_instance: 0,
            next_reservation: 0,
            next_private_ip: 0,
            vpcs: BTreeMap::new(),
            vpc_available_at: BTreeMap::new(),
            vpc_attributes: BTreeMap::new(),
            subnets: BTreeMap::new(),
            subnet_available_at: BTreeMap::new(),
            verified_access_trust_providers: BTreeMap::new(),
            verified_access_trust_provider_client_tokens: HashMap::new(),
            local_gateway_route_table_virtual_interface_group_associations: BTreeMap::new(),
            images: BTreeMap::new(),
            security_groups: BTreeMap::new(),
            launch_templates: BTreeMap::new(),
            launch_template_client_tokens: HashMap::new(),
            id_counter: 0,
            volumes: BTreeMap::new(),
            volume_client_tokens: HashMap::new(),
            volume_modifications: BTreeMap::new(),
            secondary_networks: BTreeMap::new(),
            secondary_network_client_tokens: HashMap::new(),
            byoip_cidrs: BTreeMap::new(),
            coip_pools: BTreeMap::new(),
            capacity_blocks: BTreeMap::new(),
            application_status: BTreeMap::new(),
            fast_launch_images: BTreeMap::new(),
            addresses: BTreeMap::new(),
            next_elastic_ip: 0,
            transit_gateways: BTreeMap::new(),
            transit_gateway_available_at: BTreeMap::new(),
            transit_gateway_route_tables: BTreeMap::new(),
            transit_gateway_propagations: BTreeMap::new(),
            dedicated_hosts: BTreeMap::new(),
            dedicated_host_client_tokens: HashMap::new(),
            vpc_endpoint_connections: BTreeMap::new(),
            allowed_images_settings: BTreeMap::new(),
            fpga_images: BTreeMap::new(),
            fpga_image_client_tokens: HashMap::new(),
            volumes_in_recycle_bin: BTreeMap::new(),
            image_usage_reports: BTreeMap::new(),
            export_tasks: BTreeMap::new(),
            secondary_subnets: BTreeMap::new(),
            vpc_endpoint_connection_moves: BTreeMap::new(),
            vpc_endpoint_services: BTreeMap::new(),
            vpc_endpoints: BTreeMap::new(),
            transit_gateway_attachments: BTreeMap::new(),
            transit_gateway_multicast_domains: BTreeMap::new(),
            transit_gateway_route_table_announcements: BTreeMap::new(),
            ipam_pools: BTreeMap::new(),
            vpc_block_public_access_exclusions: BTreeMap::new(),
            import_image_tasks: BTreeMap::new(),
            import_image_task_client_tokens: HashMap::new(),
            placement_groups: BTreeMap::new(),
            // "Public sharing is not blocked" is where an account that has never called
            // `EnableSnapshotBlockPublicAccess` sits.
            snapshot_block_public_access: SNAPSHOT_BLOCK_PUBLIC_ACCESS_UNBLOCKED,
        };
        let default_vpc = state.insert_vpc(
            DEFAULT_VPC_CIDR_BLOCK,
            true,
            DEFAULT_INSTANCE_TENANCY,
            VPC_STATE_AVAILABLE,
            Vec::new(),
        );
        state.insert_default_subnets(&default_vpc, DEFAULT_VPC_CIDR_BLOCK, DEFAULT_REGION);
        state.insert_default_security_group(&default_vpc, DEFAULT_REGION);
        // After the resources of the default VPC, so the ids those are minted with do not
        // depend on how many images the published catalog holds.
        state.insert_published_images();
        state
    }
}

impl ComAmazonawsEc2Amazonec2State {
    /// Mint the next id for `prefix`, e.g. `vpc-00000000000000001`.
    ///
    /// EC2 resource ids are `<prefix>-<hex>`, with 8 or 17 lowercase hex digits (the
    /// model's own id patterns read like `^vpc-([0-9a-f]{8}|[0-9a-f]{17})$`), so the
    /// suffix is 17 lowercase hex digits of the monotonic counter.
    pub fn next_id(&mut self, prefix: &str) -> String {
        self.id_counter = self.id_counter.saturating_add(1);
        format!("{prefix}-{:017x}", self.id_counter)
    }

    /// Mint an Amazon-provided IPv6 CIDR block with the `/56` prefix length
    /// `CreateVpcRequest$AmazonProvidedIpv6CidrBlock` documents ("an Amazon-provided IPv6
    /// CIDR block with a /56 prefix length for the VPC"), out of the `2600:1f00::/24`
    /// space Amazon advertises them from.
    ///
    /// The address comes from the store's own id counter, never from the request — the
    /// caller "cannot specify the range of IP addresses, or the size of the CIDR block".
    pub fn mint_amazon_ipv6_cidr(&mut self) -> String {
        self.id_counter = self.id_counter.saturating_add(1);
        let counter = self.id_counter;
        let second = u16::try_from((counter >> 24) & 0xff).unwrap_or_default();
        let third = u16::try_from((counter >> 8) & 0xffff).unwrap_or_default();
        let fourth = u16::try_from((counter & 0xff) << 8).unwrap_or_default();
        let address = Ipv6Addr::new(0x2600, 0x1f00 | second, third, fourth, 0, 0, 0, 0);
        format!("{address}/56")
    }

    /// Whether another VPC fits inside the store's ceiling ([`MAX_VPCS`]).
    ///
    /// A create must ask this BEFORE inserting and refuse with [`vpc_limit_exceeded`]
    /// when it answers `false`, the way [`Self::would_exceed_stored_bytes`] gates the
    /// caller-chosen bytes: without a count ceiling repeated creates would grow memory
    /// without bound.
    pub fn has_vpc_capacity(&self) -> bool {
        self.vpcs.len() < MAX_VPCS
    }

    /// Whether another subnet fits inside the store's ceiling ([`MAX_SUBNETS`]).
    ///
    /// The same contract as [`Self::has_vpc_capacity`]: a create asks first and refuses
    /// with EC2's `SubnetLimitExceeded` when it answers `false`.
    pub fn has_subnet_capacity(&self) -> bool {
        self.subnets.len() < MAX_SUBNETS
    }

    /// Insert a VPC with `cidr_block` as its primary IPv4 CIDR block, in `vpc_state`, and
    /// answer the `VpcId` it was given.
    ///
    /// Every id the record carries — the VPC's own, its primary CIDR association's, and
    /// the DHCP options set associated with it — is minted here, so a VPC never shares an
    /// id with another resource of this store.
    ///
    /// The caller-chosen bytes of `tags` are charged against the store's byte budget HERE
    /// rather than by each writer, so no writer can store them uncharged; a create still
    /// asks [`Self::has_vpc_capacity`] and [`Self::would_exceed_stored_bytes`] first so it
    /// can refuse instead of overshooting.
    ///
    /// The default security group a VPC comes with is NOT written here: it is
    /// [`Self::insert_default_security_group`], which every writer of a VPC calls, because
    /// that group's ARN names a Region and a VPC record carries none.
    pub fn insert_vpc(
        &mut self,
        cidr_block: &str,
        is_default: bool,
        instance_tenancy: &str,
        vpc_state: &str,
        tags: Vec<Tag>,
    ) -> String {
        let vpc_id = self.next_id("vpc");
        let association_id = self.next_id("vpc-cidr-assoc");
        let dhcp_options_id = self.next_id("dopt");
        let vpc = Vpc {
            owner_id: Some(::engine::emulator_account_id().to_string()),
            instance_tenancy: Some(instance_tenancy.to_string()),
            // No IPv6 CIDR block is associated until something associates one.
            ipv6_cidr_block_association_set: Some(Vec::new()),
            // "Information about the IPv4 CIDR blocks associated with the VPC" (`Vpc`):
            // the primary block is one of them, so it appears in both members.
            cidr_block_association_set: Some(vec![VpcCidrBlockAssociation {
                association_id: Some(association_id),
                cidr_block: Some(cidr_block.to_string()),
                cidr_block_state: Some(VpcCidrBlockState {
                    state: Some(CIDR_BLOCK_STATE_ASSOCIATED.to_string()),
                    status_message: None,
                }),
            }]),
            is_default: Some(is_default),
            // No VPC encryption control exists until one is created for the VPC.
            encryption_control: None,
            tags: Some(tags),
            block_public_access_states: Some(BlockPublicAccessStates {
                internet_gateway_block_mode: Some(BLOCK_PUBLIC_ACCESS_OFF.to_string()),
            }),
            vpc_id: Some(vpc_id.clone()),
            state: Some(vpc_state.to_string()),
            cidr_block: Some(cidr_block.to_string()),
            dhcp_options_id: Some(dhcp_options_id),
        };
        self.add_stored_bytes(tag_list_bytes(vpc.tags.as_deref().unwrap_or_default()));
        self.vpcs.insert(vpc_id.clone(), vpc);
        // The attributes the VPC starts life with, written here rather than by each
        // writer so `DescribeVpcAttribute` reads the same defaults whichever operation
        // created the VPC.
        self.vpc_attributes
            .insert(vpc_id.clone(), VpcAttributes::for_vpc(is_default));
        vpc_id
    }

    /// The attributes of one VPC, read the way `DescribeVpcAttribute` reads them: the
    /// stored entry, or the creation defaults for a VPC whose entry predates this field.
    ///
    /// A READ ONLY — a VPC missing an entry is not given one here, so describing a VPC's
    /// attributes cannot change what a later caller sees.
    pub fn vpc_attributes_of(&self, vpc_id: &str) -> Option<VpcAttributes> {
        let vpc = self.vpcs.get(vpc_id)?;
        Some(self.vpc_attributes.get(vpc_id).copied().unwrap_or_else(|| {
            // Unreachable while every writer goes through `insert_vpc`, which stores an
            // entry. The fallback reads the VPC's OWN `IsDefault` rather than assuming a
            // non-default VPC, so a VPC without an entry is still reported with the
            // defaults AWS gives a VPC of that kind.
            VpcAttributes::for_vpc(vpc.is_default.unwrap_or(false))
        }))
    }

    /// Insert a subnet, and answer the `SubnetId` it was given.
    ///
    /// Everything the service itself owns is derived here rather than taken from the
    /// caller: the id, the ARN built from it, the owning account, and
    /// `AvailableIpAddressCount` computed from the CIDR block — so two writers cannot
    /// report the same subnet differently.
    ///
    /// Like [`Self::insert_vpc`], the caller-chosen bytes of `spec.tags` are charged
    /// against the store's byte budget here, so a writer cannot store them uncharged.
    pub fn insert_subnet(&mut self, spec: SubnetSpec) -> String {
        let subnet_id = self.next_id("subnet");
        let subnet = Subnet {
            availability_zone_id: Some(spec.availability_zone_id),
            // Local network interfaces are an Outpost feature nothing here enables, so no
            // device index is reported rather than a made-up one.
            enable_lni_at_device_index: None,
            map_customer_owned_ip_on_launch: Some(false),
            // A customer-owned IPv4 pool exists only on an Outpost subnet.
            customer_owned_ipv4_pool: None,
            owner_id: Some(::engine::emulator_account_id().to_string()),
            assign_ipv6_address_on_creation: Some(false),
            // No IPv6 CIDR block is associated until something associates one.
            ipv6_cidr_block_association_set: Some(Vec::new()),
            tags: Some(spec.tags),
            subnet_arn: Some(::engine::emulator_arn(
                "ec2",
                &spec.region,
                &format!("subnet/{subnet_id}"),
            )),
            // A subnet is on an Outpost only when one was asked for.
            outpost_arn: None,
            enable_dns64: Some(false),
            ipv6_native: Some(false),
            private_dns_name_options_on_launch: Some(PrivateDnsNameOptionsOnLaunch {
                hostname_type: Some(HOSTNAME_TYPE_IP_NAME.to_string()),
                enable_resource_name_dns_a_record: Some(false),
                enable_resource_name_dns_aaaa_record: Some(false),
            }),
            block_public_access_states: Some(BlockPublicAccessStates {
                internet_gateway_block_mode: Some(BLOCK_PUBLIC_ACCESS_OFF.to_string()),
            }),
            // "Indicates if this is a subnet used with Amazon Elastic VMware Service
            // (EVS). Possible values are `Elastic VMware Service` or no value"
            // (`Subnet$Type`): no value, this being an ordinary subnet.
            r#type: None,
            subnet_id: Some(subnet_id.clone()),
            state: Some(spec.state),
            vpc_id: Some(spec.vpc_id),
            available_ip_address_count: Some(available_ipv4_addresses(&spec.cidr_block)),
            cidr_block: Some(spec.cidr_block),
            availability_zone: Some(spec.availability_zone),
            default_for_az: Some(spec.default_for_az),
            map_public_ip_on_launch: Some(spec.map_public_ip_on_launch),
        };
        self.add_stored_bytes(tag_list_bytes(subnet.tags.as_deref().unwrap_or_default()));
        self.subnets.insert(subnet_id.clone(), subnet);
        subnet_id
    }

    /// Insert a customer-owned IP (CoIP) address pool, and answer the `PoolId` it was
    /// given.
    ///
    /// The writer seam for the CoIP store, ahead of the operation that will call it:
    /// `CreateCoipPool` is still a scaffold stub with its own open issue, so for now this
    /// is reached only from this crate's tests, which seed pools the way that create will.
    /// It lives here rather than in a test so the create lands on this one record instead
    /// of a second copy of a pool beside it.
    ///
    /// Everything the service itself owns is derived here rather than taken from the
    /// caller — the id and the `PoolArn` built from it — so two writers cannot report the
    /// same pool differently, exactly as [`Self::insert_subnet`] does for a subnet. The
    /// caller-chosen bytes of the pool's CIDRs and tags are charged against the store's
    /// byte budget here, so a writer cannot store them uncharged.
    pub fn insert_coip_pool(&mut self, spec: CoipPoolSpec) -> String {
        // [`COIP_POOL_ID_PREFIX`] carries its own separator, the way
        // [`coip_pool_id_is_well_formed`] reads one, while [`Self::next_id`] adds one.
        let pool_id = self.next_id(COIP_POOL_ID_PREFIX.trim_end_matches('-'));
        let pool = crate::ops::types::CoipPool {
            pool_id: Some(pool_id.clone()),
            pool_cidrs: Some(spec.pool_cidrs),
            local_gateway_route_table_id: Some(spec.local_gateway_route_table_id),
            tags: Some(spec.tags),
            pool_arn: Some(::engine::emulator_arn(
                "ec2",
                &spec.region,
                &format!("coip-pool/{pool_id}"),
            )),
        };
        let cidr_bytes: usize = pool.pool_cidrs.iter().flatten().map(String::len).sum();
        self.add_stored_bytes(
            cidr_bytes + tag_list_bytes(pool.tags.as_deref().unwrap_or_default()),
        );
        self.coip_pools.insert(pool_id.clone(), pool);
        pool_id
    }

    /// Give `vpc_id` the default subnet AWS creates in each Availability Zone of
    /// `region`, and answer their ids in zone order.
    ///
    /// "When we create a default VPC, we create a default subnet in each Availability
    /// Zone in the Region … we add a size /20 IPv4 CIDR block" with "the auto-assign
    /// public IPv4 address attribute set to `true`" (Amazon VPC User Guide, "Default
    /// VPCs"), so each zone's subnet is the n-th [`DEFAULT_SUBNET_PREFIX_LENGTH`] block of
    /// the VPC's own CIDR block, `DefaultForAz`, and public-IP-on-launch. A zone whose
    /// block does not fit inside `vpc_cidr_block` gets no subnet, rather than one with a
    /// block outside its VPC, and a store already at [`MAX_SUBNETS`] gets none at all
    /// rather than growing past its ceiling.
    pub fn insert_default_subnets(
        &mut self,
        vpc_id: &str,
        vpc_cidr_block: &str,
        region: &str,
    ) -> Vec<String> {
        let mut inserted = Vec::new();
        for (index, zone) in availability_zones_in_region(region).into_iter().enumerate() {
            if !self.has_subnet_capacity() {
                break;
            }
            let Ok(offset) = u32::try_from(index) else {
                break;
            };
            let Some(cidr_block) =
                nth_subnet_cidr_block(vpc_cidr_block, DEFAULT_SUBNET_PREFIX_LENGTH, offset)
            else {
                continue;
            };
            inserted.push(self.insert_subnet(SubnetSpec {
                vpc_id: vpc_id.to_string(),
                cidr_block,
                availability_zone: zone.zone_name,
                availability_zone_id: zone.zone_id,
                region: region.to_string(),
                state: SUBNET_STATE_AVAILABLE.to_string(),
                default_for_az: true,
                map_public_ip_on_launch: true,
                tags: Vec::new(),
            }));
        }
        inserted
    }

    /// Store a security group, and answer the `GroupId` it was given.
    ///
    /// Everything the service itself owns is derived here rather than taken from the
    /// caller: the id, the ARN built from it, and the owning account — so two writers
    /// cannot report the same group differently.
    ///
    /// The record's caller-chosen bytes are NOT charged here, because not every writer has
    /// any: `CreateSecurityGroup` charges [`security_group_payload_bytes`] of the group it
    /// stores, while [`Self::insert_default_security_group`] writes a name and description
    /// the SERVICE chose, which the budget ([`MAX_STORED_BYTES`], a bound on caller-supplied
    /// data) does not count. [`Self::remove_security_group`] gives back on exactly the same
    /// terms.
    pub fn insert_security_group(&mut self, spec: SecurityGroupSpec) -> String {
        let group_id = self.next_id("sg");
        let group = SecurityGroup {
            group_id: Some(group_id.clone()),
            group_name: Some(spec.group_name),
            description: Some(spec.description),
            vpc_id: Some(spec.vpc_id),
            owner_id: Some(::engine::emulator_account_id().to_string()),
            security_group_arn: Some(::engine::emulator_arn(
                "ec2",
                &spec.region,
                &format!("security-group/{group_id}"),
            )),
            tags: Some(spec.tags),
            ip_permissions: Some(spec.ip_permissions),
            ip_permissions_egress: Some(spec.ip_permissions_egress),
        };
        self.security_groups.insert(group_id.clone(), group);
        group_id
    }

    /// Give `vpc_id` the default security group AWS creates with it, and answer its
    /// `GroupId`.
    ///
    /// "Your VPC automatically comes with a default security group" whose rules "allow all
    /// inbound traffic from network interfaces … that are assigned to the same security
    /// group" and "allow all outbound … traffic from the instance" (Amazon VPC User Guide,
    /// "Default security groups for your VPCs"), so the inbound rule names this group
    /// itself and the outbound one names [`ALL_IPV4_ADDRESSES`], both for every protocol.
    /// The inbound rule can only be written once the id is minted, which is why it is set
    /// after the insert rather than passed to it.
    pub fn insert_default_security_group(&mut self, vpc_id: &str, region: &str) -> String {
        let owner_id = ::engine::emulator_account_id().to_string();
        let group_id = self.insert_security_group(SecurityGroupSpec {
            vpc_id: vpc_id.to_string(),
            group_name: DEFAULT_SECURITY_GROUP_NAME.to_string(),
            description: DEFAULT_SECURITY_GROUP_DESCRIPTION.to_string(),
            region: region.to_string(),
            tags: Vec::new(),
            ip_permissions: Vec::new(),
            ip_permissions_egress: vec![allow_all_egress()],
        });
        if let Some(group) = self.security_groups.get_mut(&group_id) {
            group.ip_permissions = Some(vec![IpPermission {
                ip_protocol: Some(ALL_IP_PROTOCOLS.to_string()),
                from_port: None,
                to_port: None,
                user_id_group_pairs: Some(vec![UserIdGroupPair {
                    group_id: Some(group_id.clone()),
                    user_id: Some(owner_id),
                    // "For a security group in a nondefault VPC, use the security group
                    // ID" (`UserIdGroupPair$GroupName`): the referenced group is named by
                    // id, so no group name is reported for the pair.
                    group_name: None,
                    description: None,
                    vpc_id: None,
                    vpc_peering_connection_id: None,
                    peering_status: None,
                }]),
                ip_ranges: Some(Vec::new()),
                ipv6_ranges: Some(Vec::new()),
                prefix_list_ids: Some(Vec::new()),
            }]);
        }
        group_id
    }

    /// Store a launch template with its first version, and answer the `LaunchTemplateId` it
    /// was given.
    ///
    /// Everything the service itself owns is derived here rather than taken from the
    /// caller: the id, and the default and latest version numbers — the first version is
    /// the only version, so it is both — so two writers cannot report the same template
    /// differently. The bytes the record retains are counted into the store's budget here
    /// as well, and [`Self::remove_launch_template`] is what gives them back.
    pub fn insert_launch_template(&mut self, spec: LaunchTemplateSpec) -> String {
        let launch_template_id = self.next_id("lt");
        let payload_bytes = launch_template_payload_bytes(&spec);
        let template = LaunchTemplate {
            launch_template_id: Some(launch_template_id.clone()),
            launch_template_name: Some(spec.name),
            create_time: Some(system_time_to_date_time(spec.create_time)),
            created_by: Some(spec.created_by.clone()),
            default_version_number: Some(FIRST_LAUNCH_TEMPLATE_VERSION),
            latest_version_number: Some(FIRST_LAUNCH_TEMPLATE_VERSION),
            tags: Some(spec.tags),
            operator: spec.operator,
        };
        let mut versions = BTreeMap::new();
        versions.insert(
            FIRST_LAUNCH_TEMPLATE_VERSION,
            LaunchTemplateVersionRecord {
                version_number: FIRST_LAUNCH_TEMPLATE_VERSION,
                version_description: spec.version_description,
                create_time: spec.create_time,
                created_by: spec.created_by,
                default_version: true,
                data: spec.data,
            },
        );
        self.launch_templates.insert(
            launch_template_id.clone(),
            LaunchTemplateRecord {
                template,
                versions,
                payload_bytes,
            },
        );
        self.add_stored_bytes(payload_bytes);
        launch_template_id
    }

    /// The id of the launch template this account holds under `name`, if it holds one.
    ///
    /// A scan of the store, which [`MAX_LAUNCH_TEMPLATES`] bounds; a name names at most one
    /// template, because a create that reuses one is refused.
    pub fn launch_template_id_for_name(&self, name: &str) -> Option<String> {
        self.launch_templates
            .values()
            .find(|record| record.launch_template_name() == name)
            .map(|record| record.launch_template_id().to_string())
    }

    /// Remove the launch template `launch_template_id` names — with every version it holds,
    /// since "deleting a launch template deletes all of its versions"
    /// (`DeleteLaunchTemplate`) and they live on the one record this removes — and answer
    /// it as it was last reported.
    ///
    /// Every `CreateLaunchTemplate` client token that named it goes too: a token must never
    /// answer with a template that is no longer stored. The bytes the record held go back
    /// to the store's budget.
    pub fn remove_launch_template(
        &mut self,
        launch_template_id: &str,
    ) -> Option<LaunchTemplateRecord> {
        let record = self.launch_templates.remove(launch_template_id)?;
        self.launch_template_client_tokens
            .retain(|_, minted| minted != launch_template_id);
        self.release_stored_bytes(record.payload_bytes);
        Some(record)
    }

    /// Give `bytes` of the caller-chosen byte budget back, for a record that is no longer
    /// stored. Saturating, so the budget can never wrap below zero.
    pub fn release_stored_bytes(&mut self, bytes: usize) {
        self.stored_bytes = self.stored_bytes.saturating_sub(bytes);
    }

    // -----------------------------------------------------------------------
    // Security groups
    // -----------------------------------------------------------------------

    /// Whether another security group fits inside the store's ceiling
    /// ([`MAX_SECURITY_GROUPS`]).
    ///
    /// The same contract as [`Self::has_vpc_capacity`]: a create asks first and refuses
    /// with EC2's `SecurityGroupLimitExceeded` when it answers `false`.
    pub fn has_security_group_capacity(&self) -> bool {
        self.security_groups.len() < MAX_SECURITY_GROUPS
    }

    /// The id of the security group this account holds in `vpc_id` under `group_name`, if
    /// it holds one.
    ///
    /// "Names are case-insensitive and must be unique within the VPC"
    /// (`CreateSecurityGroupRequest$GroupName`), so the comparison folds ASCII case and a
    /// name names at most one group per VPC. A scan of the store, which
    /// [`MAX_SECURITY_GROUPS`] bounds.
    pub fn security_group_id_for_name(&self, vpc_id: &str, group_name: &str) -> Option<String> {
        self.security_groups
            .values()
            .find(|group| {
                group.vpc_id.as_deref() == Some(vpc_id)
                    && group
                        .group_name
                        .as_deref()
                        .is_some_and(|held| held.eq_ignore_ascii_case(group_name))
            })
            .and_then(|group| group.group_id.clone())
    }
}

/// The caller-chosen bytes one stored security group holds: its name, description and
/// tags. Charged against [`MAX_STORED_BYTES`] so the store is bounded in bytes and not
/// only in records.
pub fn security_group_payload_bytes(group: &SecurityGroup) -> usize {
    group.group_name.as_deref().map_or(0, str::len)
        + group.description.as_deref().map_or(0, str::len)
        + tag_list_bytes(group.tags.as_deref().unwrap_or_default())
}

/// The outbound rule every new security group carries: "a security group has an outbound
/// rule that allows all outbound traffic" (Amazon VPC User Guide, "Security group rules"),
/// which on the wire is the `-1` protocol over `0.0.0.0/0` — the same rule
/// [`ComAmazonawsEc2Amazonec2State::insert_default_security_group`] gives the default
/// group, so one definition serves both writers.
pub fn allow_all_egress() -> IpPermission {
    IpPermission {
        ip_protocol: Some(ALL_IP_PROTOCOLS.to_string()),
        // "For `tcp`, `udp`, and `icmp`, you must specify a port range"
        // (`IpPermission$IpProtocol`) — an all-protocols rule carries none.
        from_port: None,
        to_port: None,
        user_id_group_pairs: Some(Vec::new()),
        ip_ranges: Some(vec![IpRange {
            description: None,
            cidr_ip: Some(ALL_IPV4_ADDRESSES.to_string()),
        }]),
        ipv6_ranges: Some(Vec::new()),
        prefix_list_ids: Some(Vec::new()),
    }
}

/// One stored launch template: the modelled [`LaunchTemplate`] every operation that reports
/// one answers with, plus the versions the template holds.
///
/// The template is kept as the modelled shape itself — the way the VPC and subnet stores
/// keep theirs — so every member a writer sets round-trips through every reader without a
/// field-by-field copy that can silently drop one.
#[derive(Debug, Clone)]
pub struct LaunchTemplateRecord {
    /// The template as `CreateLaunchTemplate` and `DeleteLaunchTemplate` report it.
    pub template: LaunchTemplate,
    /// The template's versions, by `VersionNumber`. `CreateLaunchTemplate` stores the
    /// first one; a delete of the template takes all of them with it.
    pub versions: BTreeMap<i64, LaunchTemplateVersionRecord>,
    /// The caller-chosen bytes this record retains, counted into the store's budget while
    /// it is stored.
    pub payload_bytes: usize,
}

impl LaunchTemplateRecord {
    /// The template's own id, which is also the key the store holds it under.
    pub fn launch_template_id(&self) -> &str {
        self.template
            .launch_template_id
            .as_deref()
            .unwrap_or_default()
    }

    /// The template's name, the second key a caller may name it by.
    pub fn launch_template_name(&self) -> &str {
        self.template
            .launch_template_name
            .as_deref()
            .unwrap_or_default()
    }
}

/// One version of a stored launch template: the launch parameters the caller gave for it,
/// kept verbatim, beside what the service recorded about the version itself.
#[derive(Debug, Clone)]
pub struct LaunchTemplateVersionRecord {
    /// This version's number; the first version is [`FIRST_LAUNCH_TEMPLATE_VERSION`].
    pub version_number: i64,
    /// The description the caller gave the version, if it gave one.
    pub version_description: Option<String>,
    /// When the version was created.
    pub create_time: SystemTime,
    /// The principal that created it.
    pub created_by: String,
    /// Whether this is the template's default version.
    pub default_version: bool,
    /// The launch parameters, exactly as the request carried them, so nothing a caller set
    /// is dropped by the store.
    pub data: RequestLaunchTemplateData,
}

/// One launch template as a writer asks for it: the members a creator chooses. Everything
/// the service derives — the launch template id, and the default and latest version
/// numbers — is minted by [`ComAmazonawsEc2Amazonec2State::insert_launch_template`].
#[derive(Debug, Clone)]
pub struct LaunchTemplateSpec {
    /// The template's name, already checked against the modelled `LaunchTemplateName`
    /// constraints by [`launch_template_name_is_well_formed`].
    pub name: String,
    /// The principal that created it, as `LaunchTemplate$CreatedBy` reports it.
    pub created_by: String,
    /// When it was created.
    pub create_time: SystemTime,
    /// The tags the create asked for.
    pub tags: Vec<Tag>,
    /// The service provider that manages the template, when the request named one.
    pub operator: Option<OperatorResponse>,
    /// The description of the first version.
    pub version_description: Option<String>,
    /// The first version's launch parameters.
    pub data: RequestLaunchTemplateData,
}

/// The caller-chosen bytes one stored launch template retains.
///
/// A version keeps the WHOLE `RequestLaunchTemplateData` — dozens of members, most of them
/// caller-chosen strings and lists — so counting a handful of members by hand would leave
/// most of what the store holds unaccounted for. The `Debug` rendering visits every member
/// and its length grows with all of them, which is the measure the store's byte ceiling
/// needs: it is used to decide whether the record fits, never reported to anyone.
pub fn launch_template_payload_bytes(spec: &LaunchTemplateSpec) -> usize {
    spec.name.len()
        + spec.version_description.as_ref().map_or(0, String::len)
        + tag_list_bytes(&spec.tags)
        + format!("{:?}", spec.data).len()
}

/// Whether `id` is a well-formed launch template id.
///
/// A launch template id is `lt-` followed by 8 or 17 lowercase hexadecimal digits — the
/// short and the long id form, the same two every EC2 resource id has. EC2 answers
/// `InvalidLaunchTemplateId.Malformed` for anything else, BEFORE it looks the id up, which
/// is a different answer from `InvalidLaunchTemplateId.NotFound` for a well-formed id no
/// template was created for.
pub fn launch_template_id_is_well_formed(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix("lt-") else {
        return false;
    };
    matches!(suffix.len(), 8 | 17)
        && suffix
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Whether `name` is a name a launch template can carry: [`MIN_LAUNCH_TEMPLATE_NAME_CHARS`]
/// to [`MAX_LAUNCH_TEMPLATE_NAME_CHARS`] characters, each of them an ASCII letter, an ASCII
/// digit or one of [`LAUNCH_TEMPLATE_NAME_PUNCTUATION`].
///
/// `CreateLaunchTemplate$LaunchTemplateName` targets a plain `String` with no constraints
/// of its own, while `LaunchTemplate$LaunchTemplateName` — the member every reader reports
/// the name through — and `DeleteLaunchTemplate$LaunchTemplateName` target the modelled
/// `LaunchTemplateName`, whose `@length` is 3..128 and whose `@pattern` is
/// `^[a-zA-Z0-9\(\)\.\-/_]+$`
/// (`smithy_query(query="shape",shape="LaunchTemplateName")`). A create the boundary
/// validator lets through with a name outside that would store a template no reader could
/// report and no caller could name, so the handler refuses it here.
pub fn launch_template_name_is_well_formed(name: &str) -> bool {
    let characters = name.chars().count();
    (MIN_LAUNCH_TEMPLATE_NAME_CHARS..=MAX_LAUNCH_TEMPLATE_NAME_CHARS).contains(&characters)
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || LAUNCH_TEMPLATE_NAME_PUNCTUATION.contains(c))
}

/// Resolve the one launch template a request names, by id or by name.
///
/// "You must specify either the launch template ID or the launch template name, but not
/// both" (`DeleteLaunchTemplate$LaunchTemplateId`), so naming both is refused as a
/// parameter combination and naming neither as a missing parameter — before either is
/// looked up. An id is checked for shape first, the way EC2 orders a malformed id ahead of
/// an unknown one; a name has no shape to be wrong, so an unknown name is simply not found.
pub fn resolve_launch_template_id<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    launch_template_id: Option<&str>,
    launch_template_name: Option<&str>,
) -> Result<String, crate::scaffolded::Scaffolded<E>> {
    match (launch_template_id, launch_template_name) {
        (Some(_), Some(_)) => Err(crate::scaffolded::Scaffolded::invalid_parameter_combination(
            "Either the launch template ID or the launch template name may be specified, \
             but not both",
        )),
        (Some(id), None) => {
            if !launch_template_id_is_well_formed(id) {
                return Err(invalid_launch_template_id_malformed(id));
            }
            if !store.launch_templates.contains_key(id) {
                return Err(invalid_launch_template_id_not_found(id));
            }
            Ok(id.to_string())
        }
        (None, Some(name)) => store
            .launch_template_id_for_name(name)
            .ok_or_else(|| invalid_launch_template_name_not_found(name)),
        (None, None) => Err(crate::scaffolded::Scaffolded::missing_parameter(
            "The request must contain either the parameter LaunchTemplateId or the \
             parameter LaunchTemplateName",
        )),
    }
}

/// Refuse a create that would take the account past the number of launch templates it may
/// hold: HTTP 400 `LaunchTemplateLimitExceeded`.
pub fn launch_template_limit_exceeded<E>(
    message: impl Into<String>,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(LAUNCH_TEMPLATE_LIMIT_EXCEEDED, 400, message)
}

/// Refuse a request that named a launch template id no launch template of the account has:
/// HTTP 400 `InvalidLaunchTemplateId.NotFound`.
pub fn invalid_launch_template_id_not_found<E>(
    launch_template_id: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_LAUNCH_TEMPLATE_ID_NOT_FOUND,
        400,
        format!("The launch template ID '{launch_template_id}' does not exist"),
    )
}

/// Refuse a request that named something that is not a launch template id at all: HTTP 400
/// `InvalidLaunchTemplateId.Malformed`.
pub fn invalid_launch_template_id_malformed<E>(
    launch_template_id: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_LAUNCH_TEMPLATE_ID_MALFORMED,
        400,
        format!("Invalid launch template ID: '{launch_template_id}'"),
    )
}

/// Refuse a request that named a launch template name no launch template of the account
/// carries: HTTP 400 `InvalidLaunchTemplateName.NotFoundException`.
pub fn invalid_launch_template_name_not_found<E>(name: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_LAUNCH_TEMPLATE_NAME_NOT_FOUND,
        400,
        format!("The launch template name '{name}' does not exist"),
    )
}

/// Refuse a create that named a launch template name this account already holds a template
/// under: HTTP 400 `InvalidLaunchTemplateName.AlreadyExistsException`.
pub fn launch_template_name_already_exists<E>(name: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_LAUNCH_TEMPLATE_NAME_ALREADY_EXISTS,
        400,
        format!("The launch template name '{name}' is already in use"),
    )
}

/// One security group as a writer asks for it: the members a creator chooses. Everything
/// the service derives — the group id, its ARN and the owning account — is minted by
/// [`ComAmazonawsEc2Amazonec2State::insert_security_group`].
#[derive(Debug, Clone)]
pub struct SecurityGroupSpec {
    /// The VPC the group is in.
    pub vpc_id: String,
    /// The group's name, unique within its VPC.
    pub group_name: String,
    /// The group's description.
    pub description: String,
    /// The Region the group's ARN names.
    pub region: String,
    /// The tags the group carries.
    pub tags: Vec<Tag>,
    /// The group's inbound rules.
    pub ip_permissions: Vec<IpPermission>,
    /// The group's outbound rules.
    pub ip_permissions_egress: Vec<IpPermission>,
}

/// One subnet as a writer asks for it: the members a creator chooses. Everything the
/// service derives — the subnet id, its ARN, the owning account and the available-address
/// count — is minted by [`ComAmazonawsEc2Amazonec2State::insert_subnet`].
#[derive(Debug, Clone)]
pub struct SubnetSpec {
    /// The VPC the subnet is in.
    pub vpc_id: String,
    /// The subnet's IPv4 CIDR block.
    pub cidr_block: String,
    /// The zone name (`us-east-1a`) the subnet is in.
    pub availability_zone: String,
    /// That zone's id (`use1-az1`).
    pub availability_zone_id: String,
    /// The Region the subnet's ARN names.
    pub region: String,
    /// One value of the `SubnetState` enum.
    pub state: String,
    /// Whether this is the default subnet for its Availability Zone.
    pub default_for_az: bool,
    /// Whether instances launched here receive a public IPv4 address.
    pub map_public_ip_on_launch: bool,
    /// The tags the subnet carries.
    pub tags: Vec<Tag>,
}

/// One customer-owned IP address pool as a writer asks for it: the members a creator
/// chooses. Everything the service derives — the pool id and the `PoolArn` built from it —
/// is minted by [`ComAmazonawsEc2Amazonec2State::insert_coip_pool`].
#[derive(Debug, Clone)]
pub struct CoipPoolSpec {
    /// "The ID of the local gateway route table" (`CoipPool$LocalGatewayRouteTableId`) the
    /// pool belongs to.
    pub local_gateway_route_table_id: String,
    /// The Region the pool's ARN names.
    pub region: String,
    /// "The address ranges of the address pool" (`CoipPool$PoolCidrs`).
    pub pool_cidrs: Vec<String>,
    /// The tags the pool carries (`CoipPool$Tags`).
    pub tags: Vec<Tag>,
}

/// The number of IPv4 addresses of `cidr_block` a network interface may take: the block's
/// own size less the [`SUBNET_RESERVED_ADDRESSES`] AWS reserves in it.
///
/// A block this emulator cannot read counts no available addresses, rather than a guess.
pub fn available_ipv4_addresses(cidr_block: &str) -> i32 {
    let Some((_, prefix_length)) = parse_ipv4_cidr_block(cidr_block) else {
        return 0;
    };
    let addresses = 1u64 << (32 - u64::from(prefix_length));
    let available = addresses.saturating_sub(u64::from(SUBNET_RESERVED_ADDRESSES));
    i32::try_from(available).unwrap_or(i32::MAX)
}

/// The `index`-th block of `prefix_length` bits inside `cidr_block`, or `None` when
/// `cidr_block` is unreadable, `prefix_length` is not a longer prefix than its own, or the
/// block at `index` runs past its end.
///
/// Subnetting is this arithmetic: the parent block's base address plus `index` whole
/// child blocks, as long as that stays inside the parent.
pub fn nth_subnet_cidr_block(
    cidr_block: &str,
    prefix_length: u32,
    index: u32,
) -> Option<String> {
    let (base, parent_prefix_length) = parse_ipv4_cidr_block(cidr_block)?;
    if prefix_length < parent_prefix_length || prefix_length > 32 {
        return None;
    }
    let child_size = 1u64 << (32 - u64::from(prefix_length));
    let parent_size = 1u64 << (32 - u64::from(parent_prefix_length));
    let offset = child_size.checked_mul(u64::from(index))?;
    if offset.checked_add(child_size)? > parent_size {
        return None;
    }
    let address = u32::try_from(u64::from(base).checked_add(offset)?).ok()?;
    Some(format!(
        "{}/{prefix_length}",
        ::std::net::Ipv4Addr::from(address)
    ))
}

/// `cidr_block` as its base address and prefix length, or `None` when it is not an IPv4
/// CIDR block: the address is parsed by the standard library's own `Ipv4Addr` parser, and
/// the prefix must be a decimal 0..=32 whose host bits are clear.
fn parse_ipv4_cidr_block(cidr_block: &str) -> Option<(u32, u32)> {
    let (address, prefix_length) = cidr_block.split_once('/')?;
    let base = u32::from(address.parse::<::std::net::Ipv4Addr>().ok()?);
    let prefix_length: u32 = prefix_length.parse().ok()?;
    if prefix_length > 32 {
        return None;
    }
    // A block whose host bits are set (`172.31.0.1/16`) names no block; refusing it here
    // keeps the arithmetic above exact.
    let host_bits = 32 - prefix_length;
    if host_bits < 32 && base & ((1u32 << host_bits) - 1) != 0 {
        return None;
    }
    if host_bits == 32 && base != 0 {
        return None;
    }
    Some((base, prefix_length))
}

impl ComAmazonawsEc2Amazonec2State {
    /// A fresh, unused `r-` reservation id.
    pub fn mint_reservation_id(&mut self) -> String {
        self.next_reservation += 1;
        format!("r-{:017x}", self.next_reservation)
    }

    /// A fresh, unused `i-` instance id.
    ///
    /// The long form every instance id EC2 issues takes: `i-` and 17 lowercase
    /// hexadecimal digits of a counter that starts at one, so the id is never the null
    /// id and its leading digit is `0` — the form [`instance_id_is_well_formed`] reads,
    /// which is what lets an id outside it be answered as malformed rather than looked
    /// up.
    pub fn mint_instance_id(&mut self) -> String {
        self.next_instance += 1;
        format!("i-{:017x}", self.next_instance)
    }

    /// The next unused primary private IPv4 address, or `None` when the search is
    /// exhausted.
    ///
    /// Drawn from `172.31.0.0/16`, the CIDR AWS gives a default VPC, skipping the
    /// first four addresses of each `/24` (AWS reserves them) and the broadcast
    /// address. An address already in use is skipped in constant time, and the
    /// search probes at most [`MAX_PRIVATE_IP_PROBES`] candidates so it cannot spin.
    pub fn allocate_private_ip(&mut self) -> Option<String> {
        for _ in 0..MAX_PRIVATE_IP_PROBES {
            let host = self.next_private_ip;
            self.next_private_ip = self.next_private_ip.wrapping_add(1);
            let candidate = format!("172.31.{}.{}", (host / 251) % 256, 4 + host % 251);
            if !self.assigned_private_ips.contains(&candidate) {
                // Recorded here, not only in `insert_instance`, so a caller that
                // allocates several addresses before storing any instance cannot be
                // handed the same one twice.
                self.assigned_private_ips.insert(candidate.clone());
                return Some(candidate);
            }
        }
        None
    }

    /// Whether a stored instance already holds `address` as its primary private
    /// IPv4 address.
    pub fn private_ip_in_use(&self, address: &str) -> bool {
        self.assigned_private_ips.contains(address)
    }

    /// Give an address allocated by [`Self::allocate_private_ip`] back to the pool.
    ///
    /// A launch takes one address per instance before storing any of them, so a
    /// launch that fails part way must hand back what it took or the pool shrinks
    /// for addresses no instance ever held.
    pub fn release_private_ip(&mut self, address: &str) {
        self.assigned_private_ips.remove(address);
    }

    /// Store a launched instance, appending it to its reservation.
    pub fn insert_instance(&mut self, record: InstanceRecord) {
        if let Some(reservation) = self.reservations.get_mut(&record.reservation_id) {
            reservation.instance_ids.push(record.instance_id.clone());
        }
        self.assigned_private_ips.insert(record.private_ip.clone());
        self.stored_bytes = self
            .stored_bytes
            .saturating_add(instance_payload_bytes(&record));
        self.instances.insert(record.instance_id.clone(), record);
    }

    /// The caller-chosen bytes stored right now.
    pub fn stored_bytes(&self) -> usize {
        self.stored_bytes
    }

    /// Would storing `bytes` more caller-chosen bytes cross [`MAX_STORED_BYTES`]?
    pub fn would_exceed_stored_bytes(&self, bytes: usize) -> bool {
        self.stored_bytes.saturating_add(bytes) > MAX_STORED_BYTES
    }

    /// Account for `bytes` more caller-chosen bytes, written onto records that are
    /// already stored (a tag write).
    pub fn add_stored_bytes(&mut self, bytes: usize) {
        self.stored_bytes = self.stored_bytes.saturating_add(bytes);
    }

    /// Store a new reservation, keeping the launch order it reports in.
    pub fn insert_reservation(&mut self, record: ReservationRecord) {
        self.reservation_order.push(record.reservation_id.clone());
        self.reservations
            .insert(record.reservation_id.clone(), record);
    }

    /// Apply every clock-owed transition that has come due.
    ///
    /// A reporting operation may call this: each move was already owed at this
    /// instant whether or not anybody asked, so materialising it changes nothing
    /// a later caller would otherwise have seen. It covers every resource kind that
    /// has one — an instance's next state, a `pending` VPC becoming `available` with
    /// its IPv6 association moving `associating` -> `associated`, a `pending` subnet
    /// becoming `available`, a `pending` transit gateway with the route tables that come up
    /// with it, a VPC BPA exclusion reaching `create-complete`, a Windows fast launch
    /// configuration reaching `enabled` or finishing the cleanup a disable started, and a
    /// `pending-deprovision` BYOIP range reaching the end of its release — leaving the
    /// account, or landing in `failed-deprovision` when addresses are still allocated from
    /// it.
    ///
    /// An `active` import image task becoming `completed` with the AMI it created is one of
    /// them too; it is documented on [`Self::complete_due_import_image_tasks`], which this
    /// calls.
    pub fn materialize_due_transitions(&mut self) {
        let now = Instant::now();
        for instance in self.instances.values_mut() {
            let due = instance
                .pending_transition
                .as_ref()
                .filter(|pending| now >= pending.due_at)
                .map(|pending| pending.to);
            if let Some(state) = due {
                instance.state = state;
                instance.pending_transition = None;
            }
        }
        let vpcs = &mut self.vpcs;
        self.vpc_available_at.retain(|vpc_id, due_at| {
            if now < *due_at {
                return true;
            }
            if let Some(vpc) = vpcs.get_mut(vpc_id) {
                if vpc.state.as_deref() == Some(VPC_STATE_PENDING) {
                    vpc.state = Some(VPC_STATE_AVAILABLE.to_string());
                }
                for association in vpc.ipv6_cidr_block_association_set.iter_mut().flatten() {
                    if let Some(state) = association.ipv6_cidr_block_state.as_mut() {
                        if state.state.as_deref() == Some(CIDR_BLOCK_STATE_ASSOCIATING) {
                            state.state = Some(CIDR_BLOCK_STATE_ASSOCIATED.to_string());
                        }
                    }
                }
            }
            // The transition is applied once; nothing is owed on this VPC any more.
            false
        });
        let subnets = &mut self.subnets;
        self.subnet_available_at.retain(|subnet_id, due_at| {
            if now < *due_at {
                return true;
            }
            if let Some(subnet) = subnets.get_mut(subnet_id) {
                if subnet.state.as_deref() == Some(SUBNET_STATE_PENDING) {
                    subnet.state = Some(SUBNET_STATE_AVAILABLE.to_string());
                }
            }
            // The transition is applied once; nothing is owed on this subnet any more.
            false
        });
        // A VPC BPA exclusion `CreateVpcBlockPublicAccessExclusion` answered as
        // `create-in-progress` reaches `create-complete` once its due instant has passed,
        // whether or not anybody called — the same contract as a `pending` VPC's move
        // above.
        for record in self.vpc_block_public_access_exclusions.values_mut() {
            let due = record
                .pending
                .as_ref()
                .filter(|pending| now >= pending.due_at)
                .cloned();
            if let Some(pending) = due {
                record.view.state = Some(pending.to.to_string());
                // The exclusion changed when the move fell due, not when somebody next
                // asked about it, so "when the exclusion was last updated"
                // (`VpcBlockPublicAccessExclusion$LastUpdateTimestamp`) is the due instant.
                record.view.last_update_timestamp = Some(system_time_to_date_time(pending.at));
                record.pending = None;
            }
        }
        // The end of a release `DeprovisionByoipCidr` started: at its due instant the
        // range either stops being one of the account's ranges or lands in
        // `failed-deprovision`, whether or not anybody called again.
        //
        // Which of the two is decided HERE, at the due instant, from the addresses the
        // account holds out of the range THEN: "Ensure that all EIPs from the range have
        // been deallocated and try again" (`ByoipCidrState`'s `failed-deprovision`) is a
        // question about the end of the release, so a caller that gives an address back
        // while the range is `pending-deprovision` still gets its release.
        let due: Vec<String> = self
            .byoip_cidrs
            .iter()
            .filter(|(_, record)| {
                record
                    .released_at
                    .is_some_and(|released_at| now >= released_at)
            })
            .map(|(cidr, _)| cidr.clone())
            .collect();
        for cidr in due {
            let held = self.addresses_in_byoip_cidr(&cidr);
            if held.is_empty() {
                // The release succeeded: the range is not one of the account's any more, so
                // the bytes its record held go back to the budget — exactly what
                // [`Self::insert_byoip_cidr`] charged for it.
                if let Some(record) = self.byoip_cidrs.remove(&cidr) {
                    self.release_stored_bytes(byoip_cidr_payload_bytes(&record));
                }
                continue;
            }
            // "failed-deprovision: The request to deprovision the address range was
            // unsuccessful" (`ByoipCidrState`) — the range stays the account's, carrying
            // why in its `StatusMessage` ("Otherwise, contains an error message",
            // `ByoipCidr$StatusMessage`), and can be released again once the caller has
            // cleared what held it.
            let reason = byoip_cidr_release_failure(&held);
            if let Some(record) = self.byoip_cidrs.get_mut(&cidr) {
                record.released_at = None;
                record.byoip_cidr.state = Some(BYOIP_CIDR_STATE_FAILED_DEPROVISION.to_string());
                record.byoip_cidr.status_message = Some(reason);
            }
        }
        let transit_gateways = &mut self.transit_gateways;
        let route_tables = &mut self.transit_gateway_route_tables;
        self.transit_gateway_available_at
            .retain(|transit_gateway_id, due_at| {
                if now < *due_at {
                    return true;
                }
                // The gateway's own route tables come up with it: a route table AWS
                // reports as `pending` is `available` once the gateway is. They are
                // reached through the ids the gateway's options name, so this costs a
                // lookup per role rather than a scan of every route table the store holds.
                let mut owed: Vec<String> = Vec::new();
                if let Some(transit_gateway) = transit_gateways.get_mut(transit_gateway_id) {
                    if transit_gateway.state.as_deref() == Some(TRANSIT_GATEWAY_STATE_PENDING) {
                        transit_gateway.state = Some(TRANSIT_GATEWAY_STATE_AVAILABLE.to_string());
                    }
                    if let Some(options) = transit_gateway.options.as_ref() {
                        owed.extend(options.association_default_route_table_id.clone());
                        owed.extend(options.propagation_default_route_table_id.clone());
                    }
                }
                for route_table_id in owed {
                    if let Some(route_table) = route_tables.get_mut(&route_table_id) {
                        if route_table.state.as_deref()
                            == Some(TRANSIT_GATEWAY_ROUTE_TABLE_STATE_PENDING)
                        {
                            route_table.state =
                                Some(TRANSIT_GATEWAY_ROUTE_TABLE_STATE_AVAILABLE.to_string());
                        }
                    }
                }
                // The transition is applied once; nothing is owed on this gateway any more.
                false
            });
        self.fast_launch_images.retain(|_image_id, record| {
            let Some(pending) = record
                .pending
                .as_ref()
                .filter(|pending| now >= pending.due_at)
                .cloned()
            else {
                return true;
            };
            match pending.to {
                // "Discontinue Windows fast launch for a Windows AMI, and clean up existing
                // pre-provisioned snapshots" (`DisableFastLaunch`): once the cleanup the
                // disable started is done there is nothing left to report — the image is no
                // longer a Windows fast launch enabled image, and `FastLaunchStateCode` has
                // no member for one that is not.
                FastLaunchMove::Discontinued => false,
                FastLaunchMove::Enabled => {
                    record.view.state = Some(FAST_LAUNCH_STATE_ENABLED.to_string());
                    // The state changed when the move fell due, not when somebody asked
                    // about it, so the time reported is the due instant.
                    record.view.state_transition_time =
                        Some(system_time_to_date_time(pending.at));
                    record.pending = None;
                    true
                }
            }
        });
        // The resources whose own record carries the move the clock owes it
        // ([`PendingStateMove`]): a transit gateway attachment an acceptance or a rejection
        // left transient, one subnet association of a multicast domain, a route table
        // announcement a delete started on, and a BYOIP range whose advertisement was asked
        // for. Each move was due at its instant whether or not anybody called, so applying
        // it here changes nothing a later caller would not have seen anyway.
        for record in self.transit_gateway_attachments.values_mut() {
            if let Some(state) = record
                .pending
                .as_ref()
                .and_then(|pending| pending.due_by(now))
                .map(str::to_string)
            {
                record.view.set_state(&state);
                record.pending = None;
            }
        }
        for domain in self.transit_gateway_multicast_domains.values_mut() {
            for association in &mut domain.associations {
                if let Some(state) = association
                    .pending
                    .as_ref()
                    .and_then(|pending| pending.due_by(now))
                    .map(str::to_string)
                {
                    association.state = state;
                    association.pending = None;
                }
            }
        }
        for record in self.transit_gateway_route_table_announcements.values_mut() {
            if let Some(state) = record
                .pending
                .as_ref()
                .and_then(|pending| pending.due_by(now))
                .map(str::to_string)
            {
                record.view.state = Some(state);
                record.pending = None;
            }
        }
        for record in self.byoip_cidrs.values_mut() {
            if let Some(state) = record
                .pending
                .as_ref()
                .and_then(|pending| pending.due_by(now))
                .map(str::to_string)
            {
                record.byoip_cidr.state = Some(state);
                record.pending = None;
            }
        }
        // The end of an enable or a disable of one route propagation: the pair either
        // reaches `enabled`, or stops being a propagation of that route table at all —
        // `TransitGatewayPropagationState` has no word for a pair that does not propagate,
        // and a route table whose propagation was disabled simply does not have one any
        // more. Due at its instant whether or not anybody called, like every other move
        // above. The removals are collected first, the way the BYOIP block above collects
        // its due ranges, so each one can give its bytes back to the store's budget.
        let mut removed: Vec<(String, String)> = Vec::new();
        for (key, record) in &mut self.transit_gateway_propagations {
            let Some(due) = record
                .pending
                .as_ref()
                .filter(|pending| now >= pending.due_at)
                .map(|pending| pending.to)
            else {
                continue;
            };
            match due {
                PropagationMove::Removed => removed.push(key.clone()),
                PropagationMove::Enabled => {
                    record.state = PROPAGATION_STATE_ENABLED.to_string();
                    record.pending = None;
                }
            }
        }
        for key in removed {
            if self.transit_gateway_propagations.remove(&key).is_some() {
                // Exactly what the enable charged for the record.
                self.release_stored_bytes(STORED_RECORD_OVERHEAD_BYTES);
            }
        }
        let connections = &mut self.vpc_endpoint_connections;
        self.vpc_endpoint_connection_moves.retain(|endpoint_id, pending| {
            if now < pending.due_at {
                return true;
            }
            if let Some(connection) = connections.get_mut(endpoint_id) {
                connection.vpc_endpoint_state = Some(pending.to.clone());
            }
            // The move is applied once; nothing is owed on this connection any more.
            false
        });
        self.complete_due_import_image_tasks(now);
    }

    /// The modelled `Instance` for one stored instance.
    pub fn instance_view(&self, instance: &InstanceRecord) -> Instance {
        Instance {
            instance_id: Some(instance.instance_id.clone()),
            image_id: instance.image_id.clone(),
            instance_type: Some(instance.instance_type.clone()),
            key_name: instance.key_name.clone(),
            subnet_id: instance.subnet_id.clone(),
            // VPCs are not modelled yet, so `vpcId` stays absent rather than
            // carrying an invented id.
            state: Some(instance.state.view()),
            // EC2 reports an empty reason while the instance has not been moved
            // by a caller, so the member is always present.
            state_transition_reason: Some(
                instance.state_transition_reason.clone().unwrap_or_default(),
            ),
            state_reason: instance
                .state_reason
                .as_ref()
                .map(|(code, message)| StateReason {
                    code: Some(code.clone()),
                    message: Some(message.clone()),
                }),
            private_ip_address: Some(instance.private_ip.clone()),
            // Documented on `Instance$privateDnsName`: "This name is not available
            // until the instance enters the running state."
            private_dns_name: (instance.state != InstanceStateName::Pending)
                .then(|| instance.private_dns_name.clone()),
            public_ip_address: instance.public_ip.clone(),
            launch_time: Some(system_time_to_date_time(instance.launch_time)),
            ami_launch_index: Some(instance.ami_launch_index),
            architecture: Some(instance.architecture.clone()),
            ebs_optimized: Some(instance.ebs_optimized),
            monitoring: Some(Monitoring {
                state: Some(instance.monitoring_state.clone()),
            }),
            placement: Some(Placement {
                availability_zone: Some(instance.placement_az.clone()),
                ..Default::default()
            }),
            security_groups: Some(
                instance
                    .security_groups
                    .iter()
                    .map(|(group_id, group_name)| GroupIdentifier {
                        group_id: group_id.clone(),
                        group_name: group_name.clone(),
                    })
                    .collect(),
            ),
            client_token: instance.client_token.clone(),
            tags: Some(tag_list(&instance.tags)),
            ..Default::default()
        }
    }

    /// The modelled `Reservation` for `reservation_id`, carrying exactly
    /// `instance_ids` (which the caller has already filtered and ordered).
    pub fn reservation_view(
        &self,
        reservation_id: &str,
        instance_ids: &[String],
    ) -> Option<ReservationView> {
        let reservation = self.reservations.get(reservation_id)?;
        Some(ReservationView {
            reservation_id: Some(reservation.reservation_id.clone()),
            owner_id: Some(reservation.owner_id.clone()),
            requester_id: reservation.requester_id.clone(),
            // `Reservation$groups` is documented "Not supported", so it stays absent.
            groups: None,
            instances: Some(
                instance_ids
                    .iter()
                    .filter_map(|id| self.instances.get(id))
                    .map(|instance| self.instance_view(instance))
                    .collect(),
            ),
        })
    }

    /// The modelled `Reservation` for `reservation_id` with every instance it holds.
    pub fn whole_reservation_view(&self, reservation_id: &str) -> Option<ReservationView> {
        let instance_ids = self.reservations.get(reservation_id)?.instance_ids.clone();
        self.reservation_view(reservation_id, &instance_ids)
    }

    /// The values a filter NAME reads off one instance, or `None` when this slice
    /// does not resolve that name.
    ///
    /// A name that resolves to an empty list simply matches nothing — which is
    /// different from a name the service does not know, and which the caller
    /// refuses with `InvalidParameterValue`.
    pub fn filter_values(
        &self,
        reservation: &ReservationRecord,
        instance: &InstanceRecord,
        name: &str,
    ) -> Option<Vec<String>> {
        if let Some(key) = name.strip_prefix(TAG_FILTER_PREFIX) {
            return Some(instance.tags.get(key).cloned().into_iter().collect());
        }
        let single = |value: Option<String>| Some(value.into_iter().collect::<Vec<_>>());
        match name {
            "architecture" => single(Some(instance.architecture.clone())),
            "availability-zone" => single(Some(instance.placement_az.clone())),
            "client-token" => single(instance.client_token.clone()),
            "ebs-optimized" => single(Some(instance.ebs_optimized.to_string())),
            "image-id" => single(instance.image_id.clone()),
            "instance-id" => single(Some(instance.instance_id.clone())),
            "instance-state-code" => single(Some(instance.state.code().to_string())),
            "instance-state-name" => single(Some(instance.state.as_str().to_string())),
            "instance-type" => single(Some(instance.instance_type.clone())),
            "instance.group-id" => Some(
                instance
                    .security_groups
                    .iter()
                    .filter_map(|(group_id, _)| group_id.clone())
                    .collect(),
            ),
            "instance.group-name" => Some(
                instance
                    .security_groups
                    .iter()
                    .filter_map(|(_, group_name)| group_name.clone())
                    .collect(),
            ),
            "key-name" => single(instance.key_name.clone()),
            "launch-index" => single(Some(instance.ami_launch_index.to_string())),
            "monitoring-state" => single(Some(instance.monitoring_state.clone())),
            "owner-id" => single(Some(reservation.owner_id.clone())),
            // Matches what the instance REPORTS as its private DNS name, which is
            // nothing until it runs.
            "private-dns-name" => single(
                (instance.state != InstanceStateName::Pending)
                    .then(|| instance.private_dns_name.clone()),
            ),
            "private-ip-address" => single(Some(instance.private_ip.clone())),
            "reservation-id" => single(Some(instance.reservation_id.clone())),
            "subnet-id" => single(instance.subnet_id.clone()),
            "tag-key" => Some(instance.tags.keys().cloned().collect()),
            _ => None,
        }
    }

    /// Whether one instance satisfies EVERY filter, charging the comparisons it makes
    /// to `budget`.
    ///
    /// EC2 combines filters with AND and a single filter's values with OR, and a
    /// value may carry EC2's wildcards (see [`filter_value_matches`]). The budget is
    /// what bounds the PRODUCT of instances, filters, values and value length, none
    /// of which a per-factor cap can do on its own.
    pub fn instance_matches(
        &self,
        reservation: &ReservationRecord,
        instance: &InstanceRecord,
        filters: &[CompiledFilter],
        budget: &mut MatchBudget,
    ) -> bool {
        // One step per candidate, so even an unfiltered scan is charged and cannot
        // walk the whole store for free.
        if !budget.spend() {
            return false;
        }
        filters.iter().all(|filter| {
            let Some(held) = self.filter_values(reservation, instance, &filter.name) else {
                return false;
            };
            filter.patterns.iter().any(|pattern| {
                held.iter()
                    .any(|value| pattern.matches_within(value, budget))
            })
        })
    }
}

/// One `DescribeInstances` filter: its name and its values parsed ONCE into
/// [`FilterPattern`]s.
#[derive(Debug, Clone)]
pub struct CompiledFilter {
    pub name: String,
    pub patterns: Vec<FilterPattern>,
}

impl CompiledFilter {
    /// Parse a filter name and its values.
    pub fn new(name: String, values: Vec<String>) -> Self {
        let patterns = values
            .iter()
            .map(|value| FilterPattern::compile(value))
            .collect();
        Self { name, patterns }
    }
}

/// Whether this slice resolves `name` as a `DescribeInstances` filter.
pub fn filter_name_supported(name: &str) -> bool {
    name.starts_with(TAG_FILTER_PREFIX) || SUPPORTED_INSTANCE_FILTERS.contains(&name)
}

/// The caller-chosen bytes one stored instance holds.
///
/// Only the members whose length the REQUEST chose count: an id, a state or a
/// timestamp this emulator mints is a fixed size.
pub fn instance_payload_bytes(record: &InstanceRecord) -> usize {
    record.user_data.as_ref().map_or(0, String::len)
        + record.client_token.as_ref().map_or(0, String::len)
        + tag_bytes(&record.tags)
        + record
            .security_groups
            .iter()
            .map(|(group_id, group_name)| {
                group_id.as_ref().map_or(0, String::len)
                    + group_name.as_ref().map_or(0, String::len)
            })
            .sum::<usize>()
}

/// The bytes a tag map holds.
pub fn tag_bytes(tags: &BTreeMap<String, String>) -> usize {
    tags.iter().map(|(key, value)| key.len() + value.len()).sum()
}

/// The bytes a modelled `Tag` list holds.
pub fn tag_list_bytes(tags: &[Tag]) -> usize {
    tags.iter()
        .map(|tag| {
            tag.key.as_ref().map_or(0, String::len) + tag.value.as_ref().map_or(0, String::len)
        })
        .sum()
}

/// Merge `updates` into a modelled `Tag` list, overwriting a key already present.
///
/// The same rule `CreateTags` documents: "When you specify an existing tag key, the
/// value is overwritten with the new value."
pub fn merge_tag_list(existing: &mut Vec<Tag>, updates: &[(String, String)]) {
    for (key, value) in updates {
        match existing
            .iter_mut()
            .find(|tag| tag.key.as_deref() == Some(key.as_str()))
        {
            Some(tag) => tag.value = Some(value.clone()),
            None => existing.push(Tag {
                key: Some(key.clone()),
                value: Some(value.clone()),
            }),
        }
    }
}

/// Refuse a tag whose key or value is longer than EC2 documents.
pub fn validate_tag_size<E>(
    key: &str,
    value: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    if key.chars().count() > MAX_TAG_KEY_CHARS {
        return Err(crate::scaffolded::Scaffolded::invalid_parameter_value(format!(
            "A tag key accepts at most {MAX_TAG_KEY_CHARS} characters"
        )));
    }
    if value.chars().count() > MAX_TAG_VALUE_CHARS {
        return Err(crate::scaffolded::Scaffolded::invalid_parameter_value(format!(
            "The value of tag key '{key}' accepts at most {MAX_TAG_VALUE_CHARS} characters"
        )));
    }
    Ok(())
}

/// How many hexadecimal digits the LONG EC2 resource id form carries, the form every
/// id this service mints uses (`i-00000000000000001`): "the form
/// `i-xxxxxxxxxxxxxxxxx`" of AWS's `InvalidInstanceID.Malformed` error-code entry,
/// seventeen digits.
pub const LONG_RESOURCE_ID_DIGITS: usize = 17;

/// Whether `id` is a well-formed EC2 instance id — one EC2 could have ISSUED.
///
/// An instance id is `i-` followed by 17 lowercase hexadecimal digits — "the full
/// instance ID in the request, in the form `i-xxxxxxxxxxxxxxxxx`", as AWS's
/// `InvalidInstanceID.Malformed` error-code entry spells the only form it accepts —
/// of which the leading digit is `0` and not all are `0`. EC2 answers
/// `InvalidInstanceID.Malformed` for anything else, BEFORE it looks the id up, which
/// is a different answer from `InvalidInstanceID.NotFound` for a well-formed id
/// nothing was launched for: a client that cannot tell a typo from a missing instance
/// goes on polling for an instance that can never appear.
///
/// Three things put an id outside the form EC2 issues, so that it is malformed rather
/// than missing:
///
/// * The 8-digit SHORT id form. EC2 stopped issuing short resource ids when longer ids
///   were introduced and the short form was retired for instances, so the only
///   instance id form left is the 17-digit one AWS's error text names. Other EC2
///   resource kinds here still read both forms ([`volume_id_is_well_formed`] and the
///   rest): this is the instance id's own documented form, not a rule about every id.
/// * A LEADING DIGIT ABOVE ZERO. The long form carries 17 hexadecimal digits — 68 bits
///   — for a 64-bit internal identifier, so its leading digit is always `0`: every
///   instance id EC2 issues reads `i-0…`, which is also what this service's own mint
///   produces ([`ComAmazonawsEc2Amazonec2State::mint_instance_id`]) and what every
///   instance id seeded elsewhere in this emulator looks like. AWS answers an id whose
///   leading digit is above zero — the `i-1234567890abcdef0` shape that documentation
///   examples and hand-written probes use — with `InvalidInstanceID.Malformed`.
/// * The ALL-ZERO id, the value every client reaches for as a placeholder: AWS answers
///   `i-00000000000000000` with `InvalidInstanceID.Malformed` too, and the mint counts
///   from one, so no instance can ever hold the null id.
///
/// The structural part of this is the INSTANCE id's rule, not a rule about every EC2
/// id: the same all-zero suffix on a network interface (`eni-00000000000000000`) IS
/// answered `NotFound`, so it is not generalised to the other id readers here.
///
/// ## Where each clause's evidence is, so the rule can be re-checked
///
/// * The 17-digit form: AWS's `InvalidInstanceID.Malformed` error-code entry ("Ensure
///   that you provide the full instance ID in the request, in the form
///   `i-xxxxxxxxxxxxxxxxx`").
/// * The LEADING DIGIT: the recorded unit `ec2.op-contract-get-password-data` (run#12,
///   `assert code == "InvalidInstanceID.Malformed"`) probes a 17-hexadecimal-digit id
///   that is neither the short form nor the null id — an id whose leading digit is
///   above zero, the `i-1234567890abcdef0` shape — and AWS answered it
///   `InvalidInstanceID.Malformed`. With only the two other clauses the replay of that
///   unit still diverged with `InvalidInstanceID.NotFound`; this clause is what makes
///   the recorded answer match. The `tests/all/get_password_data.rs` rows "a leading
///   digit above zero" and "every digit above zero" pin it.
/// * The NULL id: the recorded units behind
///   `issues/.../20261022T235108Z-ec2-misclassifies-rejections-and-skips-validation-malformed-ids.md`
///   (cause 5, `i-00000000000000000` -> `InvalidInstanceID.Malformed`) and
///   `issues/.../20261022T235108Z-ec2-sql-ha-application-status-check-fast-launch.md`
///   (cause 1, the same id under `DryRun`), whose cause 7 is also where the
///   `eni-00000000000000000` -> `NotFound` counter-example comes from.
pub fn instance_id_is_well_formed(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix("i-") else {
        return false;
    };
    suffix.len() == LONG_RESOURCE_ID_DIGITS
        && suffix
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        && suffix.starts_with('0')
        && suffix.bytes().any(|byte| byte != b'0')
}

/// Refuse an instance id the request named that this service cannot resolve: a
/// malformed one before an unknown one, the way the service orders the two.
pub fn require_known_instance<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    instance_id: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    if !instance_id_is_well_formed(instance_id) {
        return Err(crate::scaffolded::Scaffolded::invalid_instance_id_malformed(
            instance_id,
        ));
    }
    if !store.instances.contains_key(instance_id) {
        return Err(crate::scaffolded::Scaffolded::invalid_instance_id_not_found(
            instance_id,
        ));
    }
    Ok(())
}

/// `ids` in request order with repeats dropped.
///
/// One instance gets one answer however many times a batch names it; processing a
/// repeat twice would report a state change from a state the caller never saw.
pub fn deduplicate_ids(ids: &[String]) -> Vec<String> {
    let mut seen = ::std::collections::HashSet::new();
    ids.iter()
        .filter(|id| seen.insert((*id).clone()))
        .cloned()
        .collect()
}

/// The modelled `Tag` list for a stored tag map, ordered by key.
pub fn tag_list(tags: &BTreeMap<String, String>) -> Vec<Tag> {
    tags.iter()
        .map(|(key, value)| Tag {
            key: Some(key.clone()),
            value: Some(value.clone()),
        })
        .collect()
}

/// The private IPv4 DNS hostname EC2 gives an instance for its primary private
/// address.
///
/// AWS builds the `ip-name` style hostname as `ip-<dash-separated address>`
/// followed by `.ec2.internal` in `us-east-1` and `.<region>.compute.internal`
/// in every other region.
pub fn private_dns_name(private_ip: &str, region: &str) -> String {
    let host = private_ip.replace('.', "-");
    if region == EC2_INTERNAL_REGION {
        format!("ip-{host}.ec2.internal")
    } else {
        format!("ip-{host}.{region}.compute.internal")
    }
}

/// The processor architecture an instance type runs on.
///
/// AWS names an instance type `<family><generation><capabilities>.<size>` and
/// defines `g` among the capability letters as "AWS Graviton processor", which is
/// `arm64`; `a1` is the first Graviton family and predates that letter, and the
/// `mac` families run Apple silicon from their second generation on. Every other
/// family is `x86_64`. The value is one of `ARCHITECTURE_VALUES_VALUES` in
/// `ops/types/shared_types.rs`.
pub fn architecture_for(instance_type: &str) -> String {
    let family = instance_type.split('.').next().unwrap_or_default();
    if family.starts_with("mac") {
        return if family == "mac1" {
            "x86_64_mac".to_string()
        } else {
            "arm64_mac".to_string()
        };
    }
    if family == "a1" {
        return "arm64".to_string();
    }
    let capabilities: String = family
        .chars()
        .skip_while(char::is_ascii_alphabetic)
        .skip_while(char::is_ascii_digit)
        .collect();
    if capabilities.contains('g') {
        "arm64".to_string()
    } else {
        "x86_64".to_string()
    }
}

/// The reason EC2 records on a state change a caller asked for, naming the
/// instant it happened.
pub fn user_initiated_reason(at: SystemTime) -> String {
    format!("User initiated ({})", iso8601_utc(at))
}

/// `at` as an RFC 3339 / ISO 8601 UTC timestamp.
pub fn iso8601_utc(at: SystemTime) -> String {
    system_time_to_date_time(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// A `SystemTime` as the modelled timestamp type.
pub fn system_time_to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let seconds = at
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    ::aws_smithy_types::DateTime::from_secs_f64(seconds)
}

/// One Availability Zone of one Region, as the describe reads it.
///
/// `zone_type`, `state` and `opt_in_status` are `&'static str` because this emulator
/// serves standard Availability Zones only: each is one fixed value of its modelled
/// enum rather than something a request can move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailabilityZoneRecord {
    /// `us-east-1a` — the Region name with the zone's letter appended.
    pub zone_name: String,
    /// `use1-az1` — the Region's zone-id prefix with the zone's number appended.
    pub zone_id: String,
    /// The Region this zone belongs to.
    pub region_name: String,
    /// The zone group this zone belongs to.
    pub group_name: String,
    /// The network border group this zone's addresses are advertised from.
    pub network_border_group: String,
    /// `availability-zone`, `local-zone` or `wavelength-zone`.
    pub zone_type: &'static str,
    /// `available`, `unavailable`, `constrained`, …
    pub state: &'static str,
    /// `opt-in-not-required`, `opted-in` or `not-opted-in`.
    pub opt_in_status: &'static str,
    /// Any messages about this zone; empty when the zone is serving normally.
    pub messages: Vec<String>,
}

impl AvailabilityZoneRecord {
    /// This zone as the modelled `AvailabilityZone` shape.
    ///
    /// `ParentZoneName`/`ParentZoneId` are absent because only a Local Zone or a
    /// Wavelength Zone has a parent zone, and `GroupLongName`, `Geography` and
    /// `SubGeography` are absent because they are display data AWS publishes per
    /// zone group that neither the model nor the request yields — reported absent
    /// rather than invented.
    pub fn to_availability_zone(&self) -> AvailabilityZone {
        AvailabilityZone {
            zone_name: Some(self.zone_name.clone()),
            zone_id: Some(self.zone_id.clone()),
            region_name: Some(self.region_name.clone()),
            group_name: Some(self.group_name.clone()),
            network_border_group: Some(self.network_border_group.clone()),
            zone_type: Some(self.zone_type.to_string()),
            state: Some(self.state.to_string()),
            opt_in_status: Some(self.opt_in_status.to_string()),
            messages: Some(
                self.messages
                    .iter()
                    .map(|message| AvailabilityZoneMessage {
                        message: Some(message.clone()),
                    })
                    .collect(),
            ),
            parent_zone_name: None,
            parent_zone_id: None,
            group_long_name: None,
            geography: None,
            sub_geography: None,
        }
    }

    /// The values the EC2 filter `name` compares against this zone, or `None` when
    /// `name` is not one of [`AVAILABILITY_ZONE_FILTERS`].
    ///
    /// A filter naming a member this zone does not carry compares against NO values,
    /// so it selects nothing — the same answer AWS gives for, say, `parent-zone-id`
    /// on an Availability Zone.
    pub fn filter_values(&self, name: &str) -> Option<Vec<&str>> {
        let values = match name {
            "group-name" => vec![self.group_name.as_str()],
            "group-long-name" => Vec::new(),
            "message" => self.messages.iter().map(String::as_str).collect(),
            "opt-in-status" => vec![self.opt_in_status],
            "parent-zone-id" | "parent-zone-name" => Vec::new(),
            "region-name" => vec![self.region_name.as_str()],
            "state" => vec![self.state],
            "zone-id" => vec![self.zone_id.as_str()],
            "zone-name" => vec![self.zone_name.as_str()],
            "zone-type" => vec![self.zone_type],
            _ => return None,
        };
        Some(values)
    }
}

/// The Availability Zones Region `region` has.
///
/// Zone `n` (counting from 1) is named `<region><letter>` with the letters running
/// from `a` — `us-east-1a` — and carries the zone id `<prefix>-az<n>` built from
/// [`zone_id_prefix`]; both forms are the ones the operation's `Filters` doc gives
/// for the `zone-name` and `zone-id` filters. Every zone is an
/// `availability-zone`, which is `opt-in-not-required`, so it is reported whether or
/// not the request sets `AllAvailabilityZones`.
pub fn availability_zones_in_region(region: &str) -> Vec<AvailabilityZoneRecord> {
    let prefix = zone_id_prefix(region);
    (0..AVAILABILITY_ZONES_PER_REGION)
        .map(|index| {
            let letter = char::from(b'a' + u8::try_from(index).unwrap_or(0));
            AvailabilityZoneRecord {
                zone_name: format!("{region}{letter}"),
                zone_id: format!("{prefix}-az{}", index + 1),
                region_name: region.to_string(),
                // The model's `group-name` filter doc gives the zone group of an
                // Availability Zone as `<region>-zg-1` (its example: `us-east-1-zg-1`).
                group_name: format!("{region}-zg-1"),
                // A standard Availability Zone's network border group is the Region.
                network_border_group: region.to_string(),
                zone_type: ZONE_TYPE_AVAILABILITY_ZONE,
                state: ZONE_STATE_AVAILABLE,
                opt_in_status: OPT_IN_NOT_REQUIRED,
                messages: Vec::new(),
            }
        })
        .collect()
}

/// A Region's zone-id prefix: its leading component, then every further component
/// abbreviated by [`abbreviate_region_component`] — `us-east-1` -> `use1`,
/// `ap-northeast-2` -> `apne2`, `us-gov-west-1` -> `usgw1`.
///
/// The form is the one the operation's `Filters` doc gives for the `zone-id` filter
/// (`use1-az1` in `us-east-1`).
pub fn zone_id_prefix(region: &str) -> String {
    let parts: Vec<&str> = region.split('-').filter(|part| !part.is_empty()).collect();
    let Some((first, rest)) = parts.split_first() else {
        return region.replace('-', "");
    };
    let mut prefix = String::from(*first);
    for part in rest {
        prefix.push_str(&abbreviate_region_component(part));
    }
    prefix
}

/// One component of a Region name as it appears in a zone id: a number is kept whole,
/// a compound cardinal direction keeps one letter per direction (`northeast` -> `ne`),
/// and anything else keeps its first character.
fn abbreviate_region_component(part: &str) -> String {
    if part.bytes().all(|byte| byte.is_ascii_digit()) {
        return part.to_string();
    }
    match part {
        "northeast" => "ne".to_string(),
        "northwest" => "nw".to_string(),
        "southeast" => "se".to_string(),
        "southwest" => "sw".to_string(),
        other => other.chars().take(1).collect(),
    }
}

/// Every filter `DescribeInstanceTypeOfferings` documents, as its `Filters` member
/// lists them
/// (`smithy_query(query="doc",shape="DescribeInstanceTypeOfferingsRequest",member="Filters")`).
/// A filter outside this set is refused; each filter inside it compares against ONE
/// field of an offering — `instance-type` against its instance type, `location`
/// against its location — which is what lets a describe narrow each field before
/// building the offerings themselves.
pub const INSTANCE_TYPE_OFFERING_FILTERS: [&str; 2] = ["instance-type", "location"];

/// The `LocationType` a `DescribeInstanceTypeOfferings` request that names none asks
/// about: the operation's own doc says "If no location is specified, the default is to
/// list the instance types that are offered in the current Region".
pub const DEFAULT_LOCATION_TYPE: &str = "region";

/// One instance type offered in one location, as the describe reads it.
///
/// `instance_type` and `location_type` are `&'static str` because each is one value of
/// its modelled enum, taken from that enum's generated value list rather than from a
/// request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceTypeOfferingRecord {
    /// `t3.micro` — a value of the modelled `InstanceType` enum.
    pub instance_type: &'static str,
    /// `region`, `availability-zone`, `availability-zone-id` or `outpost`.
    pub location_type: &'static str,
    /// The identifier of the location, in the form `location_type` selects: a Region
    /// name, a zone name, a zone id or an Outpost ARN.
    pub location: String,
}

impl InstanceTypeOfferingRecord {
    /// This offering as the modelled `InstanceTypeOffering` shape.
    pub fn to_instance_type_offering(&self) -> InstanceTypeOffering {
        InstanceTypeOffering {
            instance_type: Some(self.instance_type.to_string()),
            location_type: Some(self.location_type.to_string()),
            location: Some(self.location.clone()),
        }
    }
}

/// The modelled `LocationType` `requested` names — [`DEFAULT_LOCATION_TYPE`] when it
/// names none — or `None` when it is not one of the enum's values
/// (`crate::ops::types::LOCATION_TYPE_VALUES`, generated from the model).
pub fn location_type(requested: Option<&str>) -> Option<&'static str> {
    let requested = requested.unwrap_or(DEFAULT_LOCATION_TYPE);
    LOCATION_TYPE_VALUES
        .iter()
        .copied()
        .find(|value| *value == requested)
}

/// The locations of type `location_type` that Region `region` has.
///
/// Each form is the one the `LocationType` member's own doc gives for that value: the
/// current Region for `region`, an Availability Zone of the Region for
/// `availability-zone`, an AZ ID of the Region for `availability-zone-id` — both taken
/// from [`availability_zones_in_region`], so one zone list serves every operation —
/// and an Outpost ARN for `outpost`.
///
/// An Outpost is hardware AWS installs in a customer's own facility and no operation
/// of this service creates one, so a Region here has no Outpost locations and reports
/// no offering in one.
pub fn locations_of_type(region: &str, location_type: &str) -> Vec<String> {
    match location_type {
        "region" => vec![region.to_string()],
        "availability-zone" => availability_zones_in_region(region)
            .into_iter()
            .map(|zone| zone.zone_name)
            .collect(),
        "availability-zone-id" => availability_zones_in_region(region)
            .into_iter()
            .map(|zone| zone.zone_id)
            .collect(),
        "outpost" => Vec::new(),
        // Unreachable through [`location_type`], which answers only values of the
        // modelled enum: a value outside it names no location of any kind, so it is
        // reported as no locations rather than as the Region's.
        _ => Vec::new(),
    }
}

/// The instance types this emulator offers, in the catalog's own order.
///
/// The catalog is the modelled `InstanceType` enum's generated value list
/// (`crate::ops::types::INSTANCE_TYPE_VALUES`, from
/// `smithy_query(query="shape",shape="InstanceType")`), so an instance type is offered
/// exactly when the model declares it. AWS offers a Region-specific subset of them,
/// and neither the model nor a request carries WHICH subset a Region has, so the set
/// is the same in every Region rather than a per-Region subset invented here.
pub fn offered_instance_types() -> &'static [&'static str] {
    INSTANCE_TYPE_VALUES
}

/// Every offering of `instance_types` in `locations`, reported as `location_type`.
///
/// The caller passes the locations and the instance types that the request's filters
/// leave, which is what keeps the cost of a filtered describe proportional to
/// locations + types rather than to their product: a filter on this operation compares
/// against one field of an offering, so narrowing each field BEFORE the product is
/// built answers exactly what testing every built offering would.
///
/// The result is in cursor order — location first, then instance type, as
/// [`offering_page_cursor`] joins them — which is the order a `NextToken` walk resumes
/// in, so it is sorted here rather than inherited from the order `locations` or
/// `instance_types` happen to arrive in.
pub fn instance_type_offerings(
    locations: &[String],
    instance_types: &[&'static str],
    location_type: &'static str,
) -> Vec<InstanceTypeOfferingRecord> {
    let mut offerings: Vec<InstanceTypeOfferingRecord> = locations
        .iter()
        .flat_map(|location| {
            instance_types
                .iter()
                .map(move |instance_type| InstanceTypeOfferingRecord {
                    instance_type,
                    location_type,
                    location: location.clone(),
                })
        })
        .collect();
    offerings.sort_by_cached_key(offering_page_cursor);
    offerings
}

/// The pagination cursor naming `offering`: its location and its instance type, joined
/// by a space.
///
/// Paging is by cursor rather than by index, the way [`encode_page_token`] and
/// `DescribeVpcs` do it, so a token names the offering the next page starts at instead
/// of a position a changed filter would shift. A cursor sorts the same way
/// [`instance_type_offerings`] orders the offerings themselves — location first, then
/// instance type — because the separator sorts ahead of every character a location or
/// an instance type can carry, so one location being a prefix of another cannot
/// reorder the two.
pub fn offering_page_cursor(offering: &InstanceTypeOfferingRecord) -> String {
    format!("{} {}", offering.location, offering.instance_type)
}

// =============================================================================
// Spot placement scores
// =============================================================================

/// The Regions of the commercial (`aws`) partition, in Region-code order: the ones a
/// `GetSpotPlacementScores` request that narrows nothing is scored over.
///
/// `RegionNames` holds "the Regions used to narrow down the list of Regions to be
/// scored" (`GetSpotPlacementScoresRequest$RegionNames`), so the list it narrows has to
/// exist, and neither the Smithy model nor any operation of this emulator carries one —
/// the model's endpoint rule set resolves ONE Region name rather than enumerating them,
/// and no operation creates a Region. The catalog is therefore
/// `emulator_telemetry::policy::KNOWN_REGIONS` ("every AWS region the wire may carry",
/// already sorted for binary search) with the other partitions' Regions filtered out by
/// [`NON_COMMERCIAL_REGION_PREFIXES`] — the same source two other services derive their
/// Region catalogs from, rather than a second hand-written list that would drift from it.
/// The order it inherits is what makes [`MAX_SCORED_REGIONS`] a prefix of this list.
///
/// Nothing here is tied to one operation: `DescribeRegions` is still a scaffold stub and
/// wants exactly this catalog.
pub fn commercial_regions() -> Vec<&'static str> {
    ::emulator_telemetry::policy::KNOWN_REGIONS
        .iter()
        .copied()
        .filter(|region| {
            !NON_COMMERCIAL_REGION_PREFIXES
                .iter()
                .any(|prefix| region.starts_with(prefix))
        })
        .collect()
}

/// The region-code prefixes of the partitions that are not the commercial one: AWS China,
/// GovCloud and the isolated partitions (`us-iso`, `us-isob`, `us-isof`, `eu-isoe` — the
/// shorter prefixes cover the longer ones). They are AWS's own partition prefixes, the
/// same ones `service-com-amazonaws-observabilityadmin-observabilityadmin` partitions
/// `KNOWN_REGIONS` by.
const NON_COMMERCIAL_REGION_PREFIXES: [&str; 4] = ["cn-", "us-gov-", "us-iso", "eu-iso"];

/// The most Regions one `GetSpotPlacementScores` response scores: "The Spot placement
/// score for the top 10 Regions or Availability Zones"
/// (`GetSpotPlacementScoresResult$SpotPlacementScores`).
///
/// The ceiling is on REGIONS rather than on response items, so a request that scores
/// single Availability Zones scores the zones OF these Regions and can report more items
/// than this, paged by `MaxResults` and `NextToken`. That reading is the one the rest of
/// the model supports: the same operation publishes `MaxResults` with a range of 10..1000
/// and a `NextToken` on both its input and its output, and neither a page size above ten
/// nor a continuation token could ever do anything if one response held at most ten items.
pub const MAX_SCORED_REGIONS: usize = 10;

/// The bottom of the published score scale: "The placement score, on a scale from 1 to
/// 10. ... A score of 1 indicates that your Spot request is not likely to succeed"
/// (`SpotPlacementScore$Score`).
pub const MIN_PLACEMENT_SCORE: i32 = 1;

/// The top of the published score scale: "A score of 10 indicates that your Spot request
/// is highly likely to succeed in this Region or Availability Zone"
/// (`SpotPlacementScore$Score`).
pub const MAX_PLACEMENT_SCORE: i32 = 10;

/// The instance type count a request is diversified from: "We recommend that you specify
/// at least three instance types" (`GetSpotPlacementScoresRequest$InstanceTypes`).
pub const RECOMMENDED_INSTANCE_TYPE_COUNT: usize = 3;

/// The best score a request that names fewer than [`RECOMMENDED_INSTANCE_TYPE_COUNT`]
/// instance types gets: "If you specify one or two instance types, or specify variations
/// of a single instance type ... the returned placement score will always be low"
/// (`GetSpotPlacementScoresRequest$InstanceTypes`) — low on the 1..10 scale.
pub const UNDIVERSIFIED_PLACEMENT_SCORE: i32 = 3;

/// The placement score for `capacity` of demand placed on ONE location, given how many
/// DISTINCT instance types the request leaves Amazon EC2 to choose from (0 when it names
/// none, and so leaves EC2 to choose).
///
/// A score "reflects how likely it is that each Region or Availability Zone will succeed
/// at fulfilling the specified target capacity" (`GetSpotPlacementScoresResult$
/// SpotPlacementScores`), so it falls as the demand on a location grows and as the set of
/// instance types that could fulfil it narrows: the score starts at
/// [`MAX_PLACEMENT_SCORE`] and drops by one for each decimal order of magnitude of the
/// demand, down to [`MIN_PLACEMENT_SCORE`], and a request naming one or two instance types
/// is never scored above [`UNDIVERSIFIED_PLACEMENT_SCORE`].
///
/// The unit the capacity is counted in does not enter the score: this emulator holds no
/// vCPU-or-memory-per-instance-type data to convert `vcpu` or `memory-mib` into instances
/// with (the modelled `InstanceType` enum is a list of names — see
/// [`offered_instance_types`]), so the capacity is scored in whatever unit the request
/// states it in rather than through a conversion invented here.
pub fn placement_score(capacity: u64, instance_type_count: usize) -> i32 {
    // `ilog10` of a `u64` is at most 19, so this conversion cannot actually saturate; the
    // fallback is there so the arithmetic below needs no unchecked cast.
    let orders_of_magnitude = i32::try_from(capacity.max(1).ilog10()).unwrap_or(i32::MAX);
    let score = MAX_PLACEMENT_SCORE
        .saturating_sub(orders_of_magnitude)
        .clamp(MIN_PLACEMENT_SCORE, MAX_PLACEMENT_SCORE);
    if (1..RECOMMENDED_INSTANCE_TYPE_COUNT).contains(&instance_type_count) {
        score.min(UNDIVERSIFIED_PLACEMENT_SCORE)
    } else {
        score
    }
}

/// One scored location of a `GetSpotPlacementScores` response: a whole Region, or one
/// Availability Zone of a Region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotPlacementScoreRecord {
    /// The Region scored, or the Region of the scored Availability Zone.
    pub region: String,
    /// The zone id of the scored Availability Zone — `None` when the whole Region is
    /// scored, because then the response names no zone.
    pub availability_zone_id: Option<String>,
    /// The score, on the 1..10 scale.
    pub score: i32,
}

impl SpotPlacementScoreRecord {
    /// This record as the modelled `SpotPlacementScore` shape.
    pub fn to_spot_placement_score(&self) -> SpotPlacementScore {
        SpotPlacementScore {
            region: Some(self.region.clone()),
            availability_zone_id: self.availability_zone_id.clone(),
            score: Some(self.score),
        }
    }
}

/// Every location of `regions` scored for `capacity` and `instance_type_count`, in report
/// order: highest score first, then by location.
///
/// A request scores either whole Regions or single Availability Zones —
/// "`SingleAvailabilityZone`: specify true so that the response returns a list of scored
/// Availability Zones. Otherwise, the response returns a list of scored Regions." A
/// Region's score "assumes that your fleet request will be configured to use all
/// Availability Zones", whereas a caller scoring single zones "want[s] to launch all of
/// [its] Spot capacity into a single Availability Zone": one zone carries the whole target
/// capacity that a Region's zones would otherwise share, so a zone is scored for that
/// larger per-location demand and therefore never scores above its own Region.
pub fn spot_placement_scores(
    regions: &[String],
    capacity: u64,
    instance_type_count: usize,
    single_availability_zone: bool,
) -> Vec<SpotPlacementScoreRecord> {
    let per_zone_capacity =
        capacity.saturating_mul(u64::try_from(AVAILABILITY_ZONES_PER_REGION).unwrap_or(1));
    let mut scored: Vec<SpotPlacementScoreRecord> = regions
        .iter()
        .flat_map(|region| {
            if single_availability_zone {
                availability_zones_in_region(region)
                    .into_iter()
                    .map(|zone| SpotPlacementScoreRecord {
                        region: region.clone(),
                        availability_zone_id: Some(zone.zone_id),
                        score: placement_score(per_zone_capacity, instance_type_count),
                    })
                    .collect::<Vec<SpotPlacementScoreRecord>>()
            } else {
                vec![SpotPlacementScoreRecord {
                    region: region.clone(),
                    availability_zone_id: None,
                    score: placement_score(capacity, instance_type_count),
                }]
            }
        })
        .collect();
    scored.sort_by_cached_key(placement_score_page_cursor);
    scored
}

/// The pagination cursor naming `record`'s place in report order: its distance below the
/// top of the scale, then its Region, then its zone id, joined by spaces.
///
/// Paging is by cursor rather than by index, the way [`encode_page_token`] and
/// [`offering_page_cursor`] do it, so a token names the location the next page starts at.
/// The distance below the top of the scale comes first and fixed-width, so ascending
/// cursor order IS report order — highest score first, then location — and the separator
/// sorts ahead of every character a Region name or a zone id can carry, so one Region
/// being a prefix of another cannot reorder the two.
pub fn placement_score_page_cursor(record: &SpotPlacementScoreRecord) -> String {
    format!(
        "{:02} {} {}",
        MAX_PLACEMENT_SCORE.saturating_sub(record.score),
        record.region,
        record.availability_zone_id.as_deref().unwrap_or_default()
    )
}

/// Whether `name` is shaped like a Region code — "enter the Region code, for example,
/// us-east-1" (`GetSpotPlacementScoresRequest$RegionNames`).
///
/// A Region code is hyphen-joined lowercase groups ending in a number: `us-east-1`,
/// `ap-northeast-3`, `us-gov-west-1`. This is the grammar alone, with no allowlist: it is
/// what the Region taken from a SigV4 credential scope is held to, which is
/// caller-controlled text rendered into the zone names and zone ids a scored zone reports,
/// and that Region belongs to whichever partition the caller is calling. A Region a
/// REQUEST names is held to the catalog itself ([`commercial_regions`]) by the handler,
/// the way an instance type is held to [`offered_instance_types`].
pub fn region_name_is_well_formed(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 {
        return false;
    }
    let parts: Vec<&str> = name.split('-').collect();
    let Some((number, words)) = parts.split_last() else {
        return false;
    };
    // `us-east-1` is the shortest shape: a partition code, at least one word, then the
    // Region's number — so at least two hyphen-separated words ahead of that number.
    words.len() >= 2
        && (1..=2).contains(&number.len())
        && number.bytes().all(|byte| byte.is_ascii_digit())
        && words
            .iter()
            .all(|word| !word.is_empty() && word.bytes().all(|byte| byte.is_ascii_lowercase()))
}

/// One token of a compiled filter value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FilterToken {
    /// A literal character, whether or not it was escaped.
    Literal(char),
    /// `*`: zero or more characters.
    Many,
    /// `?`: exactly one character.
    One,
}

/// One EC2 filter value, compiled once so it can be matched against many candidates.
///
/// Filter values are case-sensitive and carry two wildcards, documented in "List and
/// filter using the CLI and API" — the guide the modelled `Filter` shape links to:
/// `*` matches zero or more characters, `?` matches exactly one, and a backslash
/// escapes either so a value can hold a literal `*` or `?`. That escaping is EC2's
/// own, which is why the matcher is written here rather than taken from a glob crate:
/// the maintained ones implement `*`/`?` without the backslash escape.
///
/// Compiling is separate from matching so a describe that compares one filter value
/// against a whole candidate set parses the value once instead of once per candidate.
#[derive(Clone, Debug)]
pub struct FilterPattern {
    tokens: Vec<FilterToken>,
    /// Whether the value was longer than [`MAX_FILTER_PATTERN_TOKENS`], in which case it
    /// was not compiled at all and matches nothing — see [`Self::compile`].
    oversize: bool,
}

/// How many tokens one compiled filter value may hold.
///
/// A filter value is request-controlled text compiled one token per character, and the
/// matcher's cost is the product of the pattern's length and the candidate's, so the token
/// count is the one factor of that product a caller could otherwise make unbounded. The
/// ceiling is [`MAX_FILTER_VALUE_BYTES`], the size every describe of this service already
/// refuses a filter value above, so a value within the documented bound always compiles
/// whole and only one that no handler would have accepted is capped here.
pub const MAX_FILTER_PATTERN_TOKENS: usize = MAX_FILTER_VALUE_BYTES;

impl FilterPattern {
    /// Compile one filter value.
    ///
    /// A value of more than [`MAX_FILTER_PATTERN_TOKENS`] tokens is NOT compiled: the
    /// pattern it would build is what makes one comparison arbitrarily expensive, so such a
    /// value matches nothing instead of being truncated to a pattern that would match MORE
    /// than the caller asked for.
    pub fn compile(pattern: &str) -> Self {
        let mut tokens: Vec<FilterToken> = Vec::new();
        let mut chars = pattern.chars();
        while let Some(c) = chars.next() {
            if tokens.len() >= MAX_FILTER_PATTERN_TOKENS {
                return Self {
                    tokens: Vec::new(),
                    oversize: true,
                };
            }
            match c {
                '\\' => match chars.next() {
                    // A trailing backslash escapes nothing, so it is itself a literal.
                    None => tokens.push(FilterToken::Literal('\\')),
                    Some(escaped) => tokens.push(FilterToken::Literal(escaped)),
                },
                '*' => tokens.push(FilterToken::Many),
                '?' => tokens.push(FilterToken::One),
                other => tokens.push(FilterToken::Literal(other)),
            }
        }
        Self {
            tokens,
            oversize: false,
        }
    }

    /// Does this filter value match `value`?
    ///
    /// Unbudgeted, for a comparison made once; a describe that scans many candidates
    /// uses [`Self::matches_within`] so one request's total work is bounded.
    pub fn matches(&self, value: &str) -> bool {
        self.matches_within(value, &mut MatchBudget::unlimited())
    }

    /// [`Self::matches`], charging every step it takes to `budget`.
    ///
    /// Answers `false` as soon as the budget is spent; the caller tells that apart
    /// from a genuine non-match with [`MatchBudget::exhausted`].
    ///
    /// EVERY path through this charges at least one step: the comparison itself on entry,
    /// one per character of the candidate the loop walks, and one per token of the pattern
    /// the trailing check walks. So no shape of filter value — an empty candidate value, a
    /// pattern that is all `*` — can do request-scaled work for free, which is what makes
    /// the budget a bound on the PRODUCT of candidates, filters, values and pattern length
    /// rather than on the candidates alone.
    pub fn matches_within(&self, value: &str, budget: &mut MatchBudget) -> bool {
        if !budget.spend() {
            return false;
        }
        // A value no handler would have accepted was never compiled, so it matches nothing.
        if self.oversize {
            return false;
        }
        let tokens = &self.tokens;
        let value: Vec<char> = value.chars().collect();
        // Greedy match with one backtrack point per `*`, which is all a pattern with no
        // alternation needs: on a mismatch, the most recent `*` consumes one more
        // character and matching resumes after it.
        let (mut token, mut position) = (0usize, 0usize);
        let mut backtrack: Option<(usize, usize)> = None;
        while position < value.len() {
            if !budget.spend() {
                return false;
            }
            let matched = match tokens.get(token) {
                Some(FilterToken::Many) => {
                    backtrack = Some((token, position));
                    token += 1;
                    continue;
                }
                Some(FilterToken::One) => true,
                Some(FilterToken::Literal(c)) => *c == value[position],
                None => false,
            };
            if matched {
                token += 1;
                position += 1;
            } else if let Some((star, consumed)) = backtrack {
                token = star + 1;
                position = consumed + 1;
                backtrack = Some((star, consumed + 1));
            } else {
                return false;
            }
        }
        // The value is exhausted: what is left of the pattern must match nothing. Each
        // remaining token is charged, so a long all-`*` tail costs what it walks even when
        // the candidate's value was empty and the loop above never ran.
        for remaining in &tokens[token.min(tokens.len())..] {
            if !budget.spend() {
                return false;
            }
            if *remaining != FilterToken::Many {
                return false;
            }
        }
        true
    }
}

/// Does the EC2 filter value `pattern` match `value`?
///
/// The one-shot form of [`FilterPattern`], for a comparison made once; a describe that
/// compares one value against many candidates should compile the pattern instead.
pub fn filter_value_matches(pattern: &str, value: &str) -> bool {
    FilterPattern::compile(pattern).matches(value)
}

/// Does a candidate resource satisfy EVERY compiled filter, charging each comparison to
/// `budget`?
///
/// The AND/OR evaluation EC2 defines for every `Filters` member, in one place: "If you
/// specify multiple filters, the filters are joined with an AND" (`Filter`), and within one
/// filter "if you specify multiple values for a filter, the values are joined with an OR"
/// (`Filter$Values`), each value matched case-sensitively with EC2's `*`/`?` wildcards. Only
/// the per-operation lookup differs, so a describe passes it as `values_of`: the values its
/// candidate holds for one filter name, or none at all when the candidate carries nothing
/// for that attribute — which then matches nothing rather than everything.
///
/// One step is charged per candidate as well, so even an unfiltered scan is counted and
/// cannot walk a whole store for free; the budget is what bounds the PRODUCT of candidates,
/// filters, values and value length, which no per-factor cap can do on its own.
pub fn matches_every_filter<'a>(
    filters: &[CompiledFilter],
    budget: &mut MatchBudget,
    values_of: &dyn Fn(&str) -> Vec<Cow<'a, str>>,
) -> bool {
    if !budget.spend() {
        return false;
    }
    filters.iter().all(|filter| {
        let held = values_of(&filter.name);
        filter.patterns.iter().any(|pattern| {
            held.iter()
                .any(|value| pattern.matches_within(value, budget))
        })
    })
}

/// Refuse a request member's value the way EC2 does: HTTP 400 `InvalidParameterValue`,
/// with a message naming the value at fault.
pub fn invalid_parameter_value<E>(message: impl Into<String>) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(INVALID_PARAMETER_VALUE, 400, message)
}

/// Refuse a request that named a VPC id no VPC of the account has, the way EC2 does:
/// HTTP 400 `InvalidVpcID.NotFound`.
pub fn invalid_vpc_id_not_found<E>(vpc_id: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_VPC_ID_NOT_FOUND,
        400,
        format!("The vpc ID '{vpc_id}' does not exist"),
    )
}

/// Refuse a request that named a subnet id no subnet of the account has, the way EC2
/// does: HTTP 400 `InvalidSubnetID.NotFound`.
pub fn invalid_subnet_id_not_found<E>(subnet_id: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_SUBNET_ID_NOT_FOUND,
        400,
        format!("The subnet ID '{subnet_id}' does not exist"),
    )
}

/// Refuse a request that named a security group no group of the account has, the way EC2
/// does: HTTP 400 `InvalidGroup.NotFound`. `group` is the id or the name the request
/// named, whichever it could not resolve.
pub fn invalid_group_not_found<E>(group: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_GROUP_NOT_FOUND,
        400,
        format!("The security group '{group}' does not exist"),
    )
}

/// Refuse a request that named an address-pool id no pool of the account has, the way
/// EC2's customer-owned-IP surface does: HTTP 400 `InvalidPoolID.NotFound`.
pub fn invalid_pool_id_not_found<E>(pool_id: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_POOL_ID_NOT_FOUND,
        400,
        format!("The pool ID '{pool_id}' does not exist"),
    )
}

/// The most characters of a refused `NextToken` its message repeats back.
///
/// The message names the token so a caller holding several can tell which one was refused;
/// it does not need the whole of an arbitrarily long one, and a bounded echo keeps a
/// request's own bytes from being reflected in full. Far above any token this service mints
/// ([`encode_page_token`]), so a real token is always named in full.
pub const MAX_ECHOED_PAGE_TOKEN_CHARS: usize = 256;

/// Refuse a `NextToken` this service did not issue: HTTP 400 `InvalidPaginationToken`,
/// with EC2's own message for it — `Next token '<token>' is invalid`, which names the
/// token the request sent, so the message is built from the request rather than fixed.
/// Only the first [`MAX_ECHOED_PAGE_TOKEN_CHARS`] characters of the token are named.
///
/// This is the form to call: a handler always has the token it is refusing, and EC2's
/// message for this refusal is a function of it. [`invalid_pagination_token`] is the
/// token-less form kept beside it, for the call sites that have not moved yet.
pub fn invalid_pagination_token_for<E>(token: &str) -> crate::scaffolded::Scaffolded<E> {
    let echoed: String = token.chars().take(MAX_ECHOED_PAGE_TOKEN_CHARS).collect();
    crate::scaffolded::Scaffolded::service_error(
        INVALID_PAGINATION_TOKEN,
        400,
        format!("Next token '{echoed}' is invalid"),
    )
}

/// Refuse a `NextToken` this service did not issue without naming it: HTTP 400
/// `InvalidPaginationToken`.
///
/// Kept so a handler landing on this helper keeps compiling;
/// [`invalid_pagination_token_for`] is what a handler should call, because the message EC2
/// answers with names the token.
pub fn invalid_pagination_token<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_PAGINATION_TOKEN,
        400,
        "The NextToken is not a token this service issued",
    )
}

/// Report that the emulator itself could not serve the request: HTTP 500
/// `InternalError`. Never for anything a request carries.
pub fn internal_error<E>(message: impl Into<String>) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(INTERNAL_ERROR, 500, message)
}

/// Answer a `DryRun` request the way EC2 does for a caller that holds the action's
/// permissions — which every caller of this emulator does: HTTP 412
/// `DryRunOperation`, and the action itself is not performed.
pub fn dry_run_operation<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        DRY_RUN_OPERATION,
        412,
        "Request would have succeeded, but DryRun flag is set.",
    )
}

// =============================================================================
// Local gateway route table / virtual interface group associations
// =============================================================================

/// The id prefix EC2 mints an association between a local gateway route table and a
/// virtual interface group with: `lgw-vif-grp-assoc-`, as the AWS Outposts and EC2
/// documentation's own ids read (for example `lgw-vif-grp-assoc-074f1e8e9bEXAMPLE`). The
/// model puts no pattern on
/// `LocalGatewayRouteTableVirtualInterfaceGroupAssociationId`, so the prefix is the one
/// thing that says which resource an id names.
pub const LOCAL_GATEWAY_VIF_GROUP_ASSOCIATION_ID_PREFIX: &str = "lgw-vif-grp-assoc";

/// How many local gateway route table / virtual interface group associations this emulator
/// stores before a write must refuse.
///
/// CHOSEN, like [`MAX_STORED_VOLUMES`] and for the same reason: nothing in this emulator
/// reclaims a stored association, so the store needs a ceiling that is not the process's
/// memory. A count ceiling is no bound on the caller-chosen bytes each record holds, so
/// those are charged against [`MAX_STORED_BYTES`] too
/// ([`local_gateway_vif_group_association_payload_bytes`]).
pub const MAX_LOCAL_GATEWAY_VIF_GROUP_ASSOCIATIONS: usize = 4096;

/// The code EC2 answers when a request would take the account past the associations it may
/// hold, in the `<Resource>LimitExceeded` family this service's other quotas are refused
/// with ([`SECONDARY_NETWORK_LIMIT_EXCEEDED`]).
///
/// No operation of this emulator writes this store yet — the operation that associates a
/// virtual interface group with a local gateway route table is still a scaffold — so this
/// code has no call site: it is the refusal the first writer must answer when
/// [`ComAmazonawsEc2Amazonec2State::insert_local_gateway_route_table_virtual_interface_group_association`]
/// hands it `None`.
pub const LOCAL_GATEWAY_VIF_GROUP_ASSOCIATION_LIMIT_EXCEEDED: &str =
    "LocalGatewayRouteTableVirtualInterfaceGroupAssociationLimitExceeded";

/// The ARN resource path of a local gateway route table:
/// `arn:aws:ec2:<region>:<account>:local-gateway-route-table/<id>`, the form
/// `LocalGatewayRouteTableVirtualInterfaceGroupAssociation$LocalGatewayRouteTableArn`
/// carries ("The Amazon Resource Name (ARN) of the local gateway route table for the
/// virtual interface group").
pub fn local_gateway_route_table_arn(region: &str, route_table_id: &str) -> String {
    ::engine::emulator_arn(
        "ec2",
        region,
        &format!("local-gateway-route-table/{route_table_id}"),
    )
}

/// What one association between a local gateway route table and a virtual interface group
/// is written with: the two resources it joins, the local gateway they belong to, the
/// Region whose ARN namespace the route table's ARN names, the association's state, and
/// its tags.
///
/// Every member the service itself owns — the association id, the route table ARN built
/// from the Region and the route table id, and the owning account — is derived by
/// [`ComAmazonawsEc2Amazonec2State::insert_local_gateway_route_table_virtual_interface_group_association`]
/// rather than taken from the caller, so two writers cannot report one association
/// differently.
#[derive(Debug, Clone)]
pub struct LocalGatewayRouteTableVirtualInterfaceGroupAssociationSpec {
    /// "The ID of the local gateway route table" the association is made in.
    pub local_gateway_route_table_id: String,
    /// "The ID of the virtual interface group" the route table is associated with.
    pub local_gateway_virtual_interface_group_id: String,
    /// "The ID of the local gateway" both belong to.
    pub local_gateway_id: String,
    /// The Region the route table's ARN names.
    pub region: String,
    /// "The state of the association"
    /// (`LocalGatewayRouteTableVirtualInterfaceGroupAssociation$State`), which the model
    /// leaves an unconstrained string.
    pub state: String,
    /// "The tags assigned to the association".
    pub tags: Vec<Tag>,
}

/// The caller-chosen bytes one stored association holds: its tags, the state a writer pinned
/// on it (`$State`, which the model leaves an unconstrained string) and the ids of the
/// resources it joins. Everything else on the record — the association id, the route table
/// ARN, the owning account — is derived by this module, so it is fixed-size.
///
/// The ONE definition of that charge, the way [`capacity_block_payload_bytes`] and
/// [`host_payload_bytes`] are for their own stores:
/// [`ComAmazonawsEc2Amazonec2State::insert_local_gateway_route_table_virtual_interface_group_association`]
/// charges exactly it, and a removal path must give back the same number.
pub fn local_gateway_vif_group_association_payload_bytes(
    spec: &LocalGatewayRouteTableVirtualInterfaceGroupAssociationSpec,
) -> usize {
    tag_list_bytes(&spec.tags)
        .saturating_add(spec.state.len())
        .saturating_add(spec.local_gateway_route_table_id.len())
        .saturating_add(spec.local_gateway_virtual_interface_group_id.len())
        .saturating_add(spec.local_gateway_id.len())
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another `count` local gateway route table / virtual interface group
    /// associations fit inside the store's ceiling
    /// ([`MAX_LOCAL_GATEWAY_VIF_GROUP_ASSOCIATIONS`]).
    ///
    /// A writer may ask this before inserting, the way [`Self::has_vpc_capacity`] and
    /// [`Self::has_dedicated_host_capacity`] gate their own stores, so it can refuse with
    /// [`LOCAL_GATEWAY_VIF_GROUP_ASSOCIATION_LIMIT_EXCEEDED`] rather than discover the
    /// refusal from an absent id. The insert itself asks it too, so no path can get past it.
    pub fn has_local_gateway_vif_group_association_capacity(&self, count: usize) -> bool {
        self.local_gateway_route_table_virtual_interface_group_associations
            .len()
            .saturating_add(count)
            <= MAX_LOCAL_GATEWAY_VIF_GROUP_ASSOCIATIONS
    }

    /// Store one association between a local gateway route table and a virtual interface
    /// group, and answer the id it was given — or `None` when the store cannot take it.
    ///
    /// Both of the store's bounds are enforced HERE, before an id is minted or a byte
    /// charged: the count ceiling
    /// ([`Self::has_local_gateway_vif_group_association_capacity`]) and the byte budget
    /// ([`Self::would_exceed_stored_bytes`] over
    /// [`local_gateway_vif_group_association_payload_bytes`]). Nothing in this emulator
    /// reclaims a stored association, so a write that is not bounded at the seam it is
    /// written through would grow the process's heap for as long as a caller keeps asking.
    /// A caller that gets `None` must refuse the request with
    /// [`LOCAL_GATEWAY_VIF_GROUP_ASSOCIATION_LIMIT_EXCEEDED`].
    pub fn insert_local_gateway_route_table_virtual_interface_group_association(
        &mut self,
        spec: LocalGatewayRouteTableVirtualInterfaceGroupAssociationSpec,
    ) -> Option<String> {
        let payload_bytes = local_gateway_vif_group_association_payload_bytes(&spec);
        if !self.has_local_gateway_vif_group_association_capacity(1)
            || self.would_exceed_stored_bytes(payload_bytes)
        {
            return None;
        }
        let association_id = self.next_id(LOCAL_GATEWAY_VIF_GROUP_ASSOCIATION_ID_PREFIX);
        let association =
            crate::ops::types::LocalGatewayRouteTableVirtualInterfaceGroupAssociation {
                local_gateway_route_table_virtual_interface_group_association_id: Some(
                    association_id.clone(),
                ),
                local_gateway_virtual_interface_group_id: Some(
                    spec.local_gateway_virtual_interface_group_id,
                ),
                local_gateway_id: Some(spec.local_gateway_id),
                local_gateway_route_table_arn: Some(local_gateway_route_table_arn(
                    &spec.region,
                    &spec.local_gateway_route_table_id,
                )),
                local_gateway_route_table_id: Some(spec.local_gateway_route_table_id),
                owner_id: Some(::engine::emulator_account_id().to_string()),
                state: Some(spec.state),
                tags: Some(spec.tags),
            };
        self.add_stored_bytes(payload_bytes);
        self.local_gateway_route_table_virtual_interface_group_associations
            .insert(association_id.clone(), association);
        Some(association_id)
    }
}

// =============================================================================
// EBS volumes
// =============================================================================

/// The `Volume` shape, under the name the scaffold generated for it.
///
/// `com.amazonaws.ec2#Volume` is both `CreateVolume`'s output shape and the member
/// type of `DescribeVolumes`'s `Volumes` list, so the generator emitted it once under
/// the output name. This alias lets the code read as the model does, the same way
/// [`ReservationView`] does for `Reservation`.
pub type VolumeView = CreateVolumeOutput;

/// A volume that is being created and cannot be attached yet: `VolumeState`'s
/// `creating` (`smithy_query(query="shape",shape="VolumeState")`).
pub const VOLUME_STATE_CREATING: &str = "creating";

/// A created volume that is not attached to an instance: `VolumeState`'s `available`.
pub const VOLUME_STATE_AVAILABLE: &str = "available";

/// A created volume that an instance holds an attachment of: `VolumeState`'s `in-use`.
pub const VOLUME_STATE_IN_USE: &str = "in-use";

/// An attachment an instance holds: `VolumeAttachmentState`'s `attached`
/// (`crate::ops::types::VOLUME_ATTACHMENT_STATE_VALUES` is the generated catalog).
pub const VOLUME_ATTACHMENT_STATE_ATTACHED: &str = "attached";

/// An attachment `DetachVolume` has started taking down: `VolumeAttachmentState`'s
/// `detaching`. The move to `detached` is owed by the clock ([`TRANSITION_DELAY`]), at
/// which point the attachment is gone and the volume is `available` again.
pub const VOLUME_ATTACHMENT_STATE_DETACHING: &str = "detaching";

/// The volume type `CreateVolume` uses when the request names none: "Default: gp2"
/// (`CreateVolumeRequest$VolumeType`).
pub const DEFAULT_VOLUME_TYPE: &str = "gp2";

/// The one resource type `CreateVolume`'s `TagSpecifications` can name: "The tags to
/// apply to the volume during creation" (`CreateVolumeRequest$TagSpecifications`).
pub const VOLUME_TAGGABLE_RESOURCE: &str = "volume";

/// How many EBS volumes this emulator stores before `CreateVolume` is refused.
///
/// CHOSEN, like [`MAX_STORED_INSTANCES`] and for the same reason: nothing in this
/// emulator reclaims a stored volume, so the store needs a ceiling that is not the
/// process's memory. It is well above any published per-Region volume quota a test
/// would exercise.
///
/// A count ceiling is not a memory bound on its own — see [`MAX_STORED_BYTES`] — so the
/// caller-chosen bytes of each volume are charged against that budget too
/// ([`volume_payload_bytes`]).
pub const MAX_STORED_VOLUMES: usize = 4096;

/// The longest `KmsKeyId` `CreateVolume` accepts.
///
/// AWS KMS bounds a key identifier — a key id, an alias, or either as an ARN — at 2048
/// characters (`KeyId` in the AWS KMS API Reference). The EC2 model declares no
/// `@length` on the member, so the handler is the only place the bound can exist, and
/// without one a single request could store an arbitrarily long string per volume.
pub const MAX_KMS_KEY_ID_CHARS: usize = 2048;

/// The size range, in GiB, each volume type accepts, as `(type, min, max)`.
///
/// Source: `CreateVolumeRequest$Size` — "Valid sizes: gp2: 1 - 16,384 GiB gp3: 1 -
/// 65,536 GiB io1: 4 - 16,384 GiB io2: 4 - 65,536 GiB st1 and sc1: 125 - 16,384 GiB
/// standard: 1 - 1024 GiB". Every value of the modelled `VolumeType` enum
/// ([`crate::ops::types::VOLUME_TYPE_VALUES`]) appears exactly once.
pub const VOLUME_SIZE_RANGES: [(&str, i32, i32); 7] = [
    ("gp2", 1, 16_384),
    ("gp3", 1, 65_536),
    ("io1", 4, 16_384),
    ("io2", 4, 65_536),
    ("sc1", 125, 16_384),
    ("st1", 125, 16_384),
    ("standard", 1, 1_024),
];

/// The size range `volume_type` accepts, or `None` for a type the model does not
/// publish one for.
pub fn volume_size_range(volume_type: &str) -> Option<(i32, i32)> {
    VOLUME_SIZE_RANGES
        .iter()
        .find(|(name, _, _)| *name == volume_type)
        .map(|(_, min, max)| (*min, *max))
}

/// The IOPS range each volume type that provisions IOPS accepts, as
/// `(type, min, max)`.
///
/// Source: `CreateVolumeRequest$Iops` — "Required for io1 and io2 volumes. Optional
/// for gp3 volumes. Omit for all other volume types. Valid ranges: gp3: 3,000
/// (default) - 80,000 IOPS io1: 100 - 64,000 IOPS io2: 100 - 256,000 IOPS". A type
/// absent from this table takes no `Iops` at all.
pub const VOLUME_IOPS_RANGES: [(&str, i32, i32); 3] =
    [("gp3", 3_000, 80_000), ("io1", 100, 64_000), ("io2", 100, 256_000)];

/// The IOPS range `volume_type` accepts, or `None` for a type that takes none.
pub fn volume_iops_range(volume_type: &str) -> Option<(i32, i32)> {
    VOLUME_IOPS_RANGES
        .iter()
        .find(|(name, _, _)| *name == volume_type)
        .map(|(_, min, max)| (*min, *max))
}

/// The volume types `Iops` is REQUIRED for: "Required for io1 and io2 volumes"
/// (`CreateVolumeRequest$Iops`) — and the same two types Multi-Attach is supported
/// with: "This parameter is supported with io1 and io2 volumes only"
/// (`CreateVolumeRequest$MultiAttachEnabled`).
pub const PROVISIONED_IOPS_VOLUME_TYPES: [&str; 2] = ["io1", "io2"];

/// The IOPS a `gp3` volume gets when the request names none: "gp3: 3,000 (default)"
/// (`CreateVolumeRequest$Iops`).
pub const DEFAULT_GP3_IOPS: i32 = 3_000;

/// The one volume type `Throughput` applies to: "Supported for gp3 volumes only. Omit
/// for all other volume types" (`CreateVolumeRequest$Throughput`).
pub const THROUGHPUT_VOLUME_TYPE: &str = "gp3";

/// The throughput range a `gp3` volume accepts, in MiB/s: "Valid Range: 125 - 2000
/// MiB/s" (`CreateVolumeRequest$Throughput`).
pub const GP3_THROUGHPUT_RANGE: (i32, i32) = (125, 2_000);

/// The throughput a `gp3` volume gets when the request names none: the bottom of
/// [`GP3_THROUGHPUT_RANGE`], which is what AWS provisions by default.
pub const DEFAULT_GP3_THROUGHPUT: i32 = GP3_THROUGHPUT_RANGE.0;

/// One stored EBS volume: the modelled [`VolumeView`] every operation that reports
/// one answers with, plus the instant the clock owes it its `creating` ->
/// `available` move.
///
/// The view holds no `State`: the state is a fact of the volume's age, so
/// [`VolumeRecord::view_at`] derives it rather than letting two writers store two
/// different words for it.
#[derive(Debug, Clone)]
pub struct VolumeRecord {
    /// The volume as the model shapes it, with `State` left for `view_at` to fill.
    pub view: VolumeView,
    /// The instant from which this volume's `creating` -> `available` move is due.
    /// Monotonic ([`Instant`]), because it is only ever compared against the clock
    /// and never reported, so a wall-clock adjustment cannot move a volume back to
    /// `creating`.
    pub available_at: Instant,
    /// Whether this volume's I/O operations are enabled.
    ///
    /// Amazon EBS disables I/O for a volume "because the data on the volume was
    /// potentially inconsistent" (`EnableVolumeIO`), and `EnableVolumeIO` is the call that
    /// turns it back on — so that operation's precondition is a fact OF THE VOLUME, held
    /// here because the modelled `Volume` shape carries no member for it (it is reported
    /// by `DescribeVolumeStatus`'s `io-enabled` check). Nothing in this emulator finds a
    /// volume's data inconsistent, so a volume is stored with its I/O enabled and the
    /// enable is refused as out of state until something disables it.
    pub io_enabled: bool,
    /// For each attachment of this volume that is being taken down, the instant its
    /// `detaching` -> `detached` move falls due, keyed by the `(InstanceId, Device)` pair
    /// that identifies the attachment — an instance can hold the same Multi-Attach volume
    /// at more than one device, so the instance alone does not name one attachment.
    ///
    /// `DetachVolume` answers an attachment in the non-terminal `detaching` state, so the
    /// clock owes it `detached` [`TRANSITION_DELAY`] later whether or not anybody calls
    /// again — the same span, read off the same monotonic clock, as this volume's
    /// `creating` -> `available` move. A detached attachment is not a state of an
    /// attachment the volume still holds: it is the attachment being gone, so
    /// [`VolumeRecord::attachments_at`] drops it once the move is due rather than
    /// reporting a `detached` row for ever, and the next writer materialises that removal
    /// ([`VolumeRecord::materialize_due_detachments`]) so no entry outlives the attachment
    /// it was made for.
    pub detaching_at: BTreeMap<(String, String), Instant>,
}

impl VolumeRecord {
    /// A volume this store has just created: attached to nothing, with its I/O enabled,
    /// reaching `available` at `available_at`.
    pub fn created(view: VolumeView, available_at: Instant) -> Self {
        Self {
            view,
            available_at,
            io_enabled: true,
            detaching_at: BTreeMap::new(),
        }
    }

    /// This volume as of `now`: the stored view with the `State` the clock has
    /// reached and the attachments it still holds. A read observes the transitions the
    /// clock already owed without writing anything.
    pub fn view_at(&self, now: Instant) -> VolumeView {
        let mut view = self.view.clone();
        view.attachments = Some(self.attachments_at(now));
        view.state = Some(self.state_at(now).to_string());
        view
    }

    /// The attachments this volume holds at `now`: every stored attachment except one
    /// whose `detaching` -> `detached` move the clock has already made due.
    pub fn attachments_at(&self, now: Instant) -> Vec<AttachVolumeOutput> {
        self.view
            .attachments
            .iter()
            .flatten()
            .filter(|attachment| !self.detach_is_due(attachment, now))
            .cloned()
            .collect()
    }

    /// Whether the detach of `attachment` was owed by `now`.
    fn detach_is_due(&self, attachment: &AttachVolumeOutput, now: Instant) -> bool {
        self.detaching_at
            .get(&Self::attachment_key(attachment))
            .is_some_and(|due_at| now >= *due_at)
    }

    /// The `(InstanceId, Device)` pair that identifies `attachment` among the attachments
    /// of one volume — the ONE reading of that key, so a writer and a reader cannot key
    /// the same attachment two ways.
    pub fn attachment_key(attachment: &AttachVolumeOutput) -> (String, String) {
        (
            attachment.instance_id.clone().unwrap_or_default(),
            attachment.device.clone().unwrap_or_default(),
        )
    }

    /// Apply the detachments the clock has already made due: the attachments they took
    /// down are no longer the volume's, and nothing is owed on them any more.
    ///
    /// A WRITER calls this, which is the only thing that may materialise it — a report
    /// observes the same removal through [`VolumeRecord::attachments_at`] without writing.
    /// It keeps the stored record converging on what readers already see, so no entry
    /// outlives the attachment it was made for and a later attachment of the same volume
    /// to the same device cannot inherit a past-due one.
    pub fn materialize_due_detachments(&mut self, now: Instant) {
        self.detaching_at.retain(|_, due_at| now < *due_at);
        let owed = &self.detaching_at;
        if let Some(attachments) = self.view.attachments.as_mut() {
            attachments.retain(|attachment| {
                owed.contains_key(&(
                    attachment.instance_id.clone().unwrap_or_default(),
                    attachment.device.clone().unwrap_or_default(),
                )) || attachment.state.as_deref() != Some(VOLUME_ATTACHMENT_STATE_DETACHING)
            });
        }
    }

    /// The volume's `VolumeState` at `now`.
    ///
    /// A volume being created is `creating`; one an instance still holds an attachment of
    /// is `in-use` ("The volume state", `Volume$State`, over `VolumeState`'s members); and
    /// one attached to nothing is `available`.
    pub fn state_at(&self, now: Instant) -> &'static str {
        if now < self.available_at {
            VOLUME_STATE_CREATING
        } else if self.attachments_at(now).is_empty() {
            VOLUME_STATE_AVAILABLE
        } else {
            VOLUME_STATE_IN_USE
        }
    }

    /// This volume as `CreateVolume` reports it: [`VolumeRecord::view_at`] without
    /// the members the model says that operation does not return — "This parameter
    /// is not returned by CreateVolume" on `Attachments`, `FastRestored` and
    /// `SseType`.
    pub fn create_response(&self, now: Instant) -> VolumeView {
        VolumeView {
            attachments: None,
            fast_restored: None,
            sse_type: None,
            ..self.view_at(now)
        }
    }
}

/// The caller-chosen bytes one stored volume holds: its tags, the KMS key the request
/// named and the `ClientToken` the token map keeps for it. Everything else on the record
/// — the id, the ARN, the account, the zone, the sizes and the states — is derived by
/// this module or bounded by the model, so it is fixed-size.
///
/// The ONE definition of that charge: `CreateVolume` asks it before the write and charges
/// exactly it, and a removal path must give back the same number.
pub fn volume_payload_bytes(
    tags: &[Tag],
    kms_key_id: Option<&str>,
    client_token: Option<&str>,
) -> usize {
    tag_list_bytes(tags)
        .saturating_add(kms_key_id.map_or(0, str::len))
        .saturating_add(client_token.map_or(0, str::len))
}

/// Whether `id` is a well-formed EBS volume id.
///
/// A volume id is `vol-` followed by 8 or 17 lowercase hexadecimal digits — the short
/// and the long id form, as the model's own `^vol-([0-9a-f]{8}|[0-9a-f]{17})$` style
/// id patterns read. EC2 answers `InvalidVolumeID.Malformed` for anything else,
/// BEFORE it looks the id up.
///
/// The rule itself is [`prefixed_id_is_well_formed`], which every resource of this module
/// asks, so the EC2 id form is defined in exactly one place; only the prefix is this
/// function's own.
pub fn volume_id_is_well_formed(id: &str) -> bool {
    prefixed_id_is_well_formed("vol", id)
}

/// Refuse a volume id the request named that this service cannot resolve: a malformed
/// one before an unknown one, the way the service orders the two.
pub fn require_known_volume<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    volume_id: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    if !volume_id_is_well_formed(volume_id) {
        return Err(invalid_volume_id_malformed(volume_id));
    }
    if !store.volumes.contains_key(volume_id) {
        return Err(invalid_volume_id_not_found(volume_id));
    }
    Ok(())
}

/// Refuse a request that named something that is not a volume id at all: HTTP 400
/// `InvalidVolumeID.Malformed`, which EC2 answers BEFORE it looks the id up.
///
/// The ONE wording of that refusal: [`require_known_volume`] answers it for an operation
/// that resolves the volume straight away, and an operation that answers its `DryRun`
/// probe between the two halves of the rule calls this and
/// [`invalid_volume_id_not_found`] itself.
pub fn invalid_volume_id_malformed<E>(volume_id: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_VOLUME_ID_MALFORMED,
        400,
        format!("Invalid id: '{volume_id}'"),
    )
}

/// Refuse a request that named a volume id no volume of the account has, the way EC2
/// does: HTTP 400 `InvalidVolume.NotFound`.
pub fn invalid_volume_id_not_found<E>(volume_id: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_VOLUME_ID_NOT_FOUND,
        400,
        format!("The volume '{volume_id}' does not exist."),
    )
}

// ---------------------------------------------------------------------------
// VM import: the import image tasks `ImportImage` starts
// ---------------------------------------------------------------------------

// The modelled shapes this slice reads, named HERE rather than in the module's shared
// `use crate::ops::types::{..}` list at the top: that list is edited by every store this
// crate gains, so naming them beside the code that uses them keeps this slice off a line
// nothing else of it touches.
use crate::ops::types::{ImportImageOutput, ImportImageTask};

/// The resource type an `ImportImage` `TagSpecification` may name: "The tags to apply to
/// the import image task during creation" (`ImportImageRequest$TagSpecifications`), which
/// the modelled `ResourceType` enum spells `import-image-task`.
pub const IMPORT_IMAGE_TASK_TAGGABLE_RESOURCE: &str = "import-image-task";

/// The id prefix of an import image task, the form `ImportImageTask$ImportTaskId` carries.
pub const IMPORT_IMAGE_TASK_ID_PREFIX: &str = "import-ami";

/// [`IMPORT_IMAGE_TASK_ID_PREFIX`] and the hyphen that separates it from the id's hex
/// suffix, as one static, so recognising an id allocates nothing on the request path.
pub const IMPORT_IMAGE_TASK_ID_PREFIX_WITH_HYPHEN: &str = "import-ami-";

/// How many disk containers one `ImportImage` request may name.
///
/// CHOSEN, like [`MAX_SECURITY_GROUPS_PER_INSTANCE`]: neither the model nor the request
/// parsers bound the length of `DiskContainers` (the list carries no `@length` and the
/// query parser bounds nesting depth only), while every container retains a
/// `SnapshotDetail` on the stored task for the process's lifetime. It is well above the
/// disks of any VM a test would import.
pub const MAX_IMPORT_DISK_CONTAINERS: usize = 32;

/// How many license configurations one `ImportImage` request may name, for the same reason
/// and in the same style as [`MAX_IMPORT_DISK_CONTAINERS`]: the modelled list is unbounded
/// and every entry is retained on the stored task.
pub const MAX_IMPORT_LICENSE_SPECIFICATIONS: usize = 32;

/// What one element of a stored import task's caller-chosen lists costs against the store's
/// byte budget beyond the strings it carries.
///
/// A `SnapshotDetail` or a license specification whose every string is empty still retains
/// a record, so charging string lengths alone would let a request store many elements for
/// nothing. The number is the order of the stored struct, not a measurement of it — it only
/// has to make the per-element cost non-zero.
pub const IMPORT_TASK_ELEMENT_OVERHEAD_BYTES: usize = 256;

/// The state of an import task that is still running.
///
/// Source for this and the three below: `DescribeImportImageTasksRequest$Filters` —
/// "Filter tasks using the task-state filter and one of the following values: active,
/// completed, deleting, or deleted."
pub const IMPORT_TASK_STATE_ACTIVE: &str = "active";

/// The state of an import task whose AMI has been created.
pub const IMPORT_TASK_STATE_COMPLETED: &str = "completed";

/// The state of an import task a cancel is working through.
pub const IMPORT_TASK_STATE_DELETING: &str = "deleting";

/// The state of an import task a cancel has finished with.
pub const IMPORT_TASK_STATE_DELETED: &str = "deleted";

/// Every state an import task reports, the catalog the `task-state` filter selects over.
pub const IMPORT_TASK_STATES: [&str; 4] = [
    IMPORT_TASK_STATE_ACTIVE,
    IMPORT_TASK_STATE_COMPLETED,
    IMPORT_TASK_STATE_DELETING,
    IMPORT_TASK_STATE_DELETED,
];

/// The architectures `ImportImage` accepts: "Valid values: i386 | x86_64"
/// (`ImportImageRequest$Architecture`).
pub const IMPORT_IMAGE_ARCHITECTURES: [&str; 2] = ["i386", "x86_64"];

/// The target hypervisors `ImportImage` accepts: "Valid values: xen"
/// (`ImportImageRequest$Hypervisor`), which is therefore also the one a task that named
/// none reports.
pub const IMPORT_IMAGE_HYPERVISORS: [&str; 1] = [IMAGE_HYPERVISOR_XEN];

/// The operating systems `ImportImage` accepts: "Valid values: Windows | Linux"
/// (`ImportImageRequest$Platform`).
pub const IMPORT_IMAGE_PLATFORMS: [&str; 2] = ["Linux", IMAGE_PLATFORM_WINDOWS];

/// The license type that replaces the source-system license: "Specify AWS to replace the
/// source-system license with an Amazon Web Services license" (`ImportImageRequest$LicenseType`).
pub const IMPORT_IMAGE_LICENSE_TYPE_AWS: &str = "AWS";

/// The license type that retains the source-system license: "or BYOL to retain the
/// source-system license" (`ImportImageRequest$LicenseType`).
pub const IMPORT_IMAGE_LICENSE_TYPE_BYOL: &str = "BYOL";

/// The license types `ImportImage` accepts.
pub const IMPORT_IMAGE_LICENSE_TYPES: [&str; 2] = [
    IMPORT_IMAGE_LICENSE_TYPE_AWS,
    IMPORT_IMAGE_LICENSE_TYPE_BYOL,
];

/// The disk image formats a disk container may carry: "Valid values: OVA | VHD | VHDX |
/// VMDK | RAW" (`ImageDiskContainer$Format`).
pub const IMPORT_DISK_IMAGE_FORMATS: [&str; 5] = ["OVA", "VHD", "VHDX", "VMDK", "RAW"];

/// The status message an import image task carries while its disks are being converted,
/// which is the whole of the window this emulator's import takes.
pub const IMPORT_TASK_STATUS_MESSAGE_CONVERTING: &str = "converting";

/// The highest percentage an ACTIVE import task reports: a task that has not finished has
/// not reached 100, and 100 is only ever read off a `completed` one.
pub const MAX_ACTIVE_IMPORT_PROGRESS: u64 = 99;

/// How many import image tasks this emulator stores before `ImportImage` is refused.
///
/// CHOSEN, like [`MAX_STORED_VOLUMES`] and for the same reason: nothing in this emulator
/// reclaims a finished task, so the store needs a ceiling that is not the process's
/// memory. A count ceiling is not a memory bound on its own — see [`MAX_STORED_BYTES`] —
/// so the caller-chosen bytes of each task are charged against that budget too
/// ([`import_image_task_payload_bytes`]).
pub const MAX_STORED_IMPORT_IMAGE_TASKS: usize = 4_096;

/// EC2's generic resource-count quota code, which a refusal to store another resource of a
/// kind with no quota code of its own answers with.
pub const RESOURCE_COUNT_EXCEEDED: &str = "ResourceCountExceeded";

/// EC2's code for a conversion (import) task id it cannot resolve — the code
/// `DescribeImportImageTasks` answers for an `ImportTaskIds` entry that is not an import
/// task id, and for one no task of the account has.
pub const INVALID_CONVERSION_TASK_ID: &str = "InvalidConversionTaskId";

/// One stored import image task: the modelled [`ImportImageTask`] every operation that
/// reports one answers with, plus the window the import takes.
///
/// The stored view is the truth about the task: its `Status` starts at
/// [`IMPORT_TASK_STATE_ACTIVE`] and the clock-owed completion is written onto it ONCE by
/// [`ComAmazonawsEc2Amazonec2State::complete_due_import_image_tasks`], because completing
/// MINTS resources (the AMI, and a snapshot id per imported disk) that have to be the same
/// in every later read. Only the progress PERCENTAGE is derived per read
/// ([`Self::view_at`]): it is a fact of elapsed time, so no stored value could be right
/// twice.
#[derive(Debug, Clone)]
pub struct ImportImageTaskRecord {
    /// The task as the model shapes it, with `Progress` left for [`Self::view_at`] to fill
    /// while the task is active.
    pub view: ImportImageTask,
    /// When the import started, which progress is measured from.
    pub started_at: Instant,
    /// The instant from which this task's `active` -> `completed` move is due. Monotonic
    /// ([`Instant`]), because it is only ever compared against the clock and never
    /// reported, so a wall-clock adjustment cannot move a task back to `active`.
    pub completed_at: Instant,
}

impl ImportImageTaskRecord {
    /// This task as of `now`: the stored view, with the percentage of progress an active
    /// import has made by `now` on the task and on each of its disks. A read observes it
    /// without writing anything.
    pub fn view_at(&self, now: Instant) -> ImportImageTask {
        let mut view = self.view.clone();
        // A task that is no longer active has no progress left to report: "The percentage
        // of progress of the import image task" (`ImportImageTask$Progress`).
        if view.status.as_deref() != Some(IMPORT_TASK_STATE_ACTIVE) {
            return view;
        }
        let progress = self.progress_at(now);
        for detail in view.snapshot_details.iter_mut().flatten() {
            detail.progress = Some(progress.clone());
        }
        view.progress = Some(progress);
        view
    }

    /// The percentage of progress this import has made by `now`, as the string the
    /// `Progress` members carry.
    ///
    /// Derived from the share of the import's window that has elapsed, and held below
    /// [`MAX_ACTIVE_IMPORT_PROGRESS`] because a task still reporting progress has not
    /// finished.
    pub fn progress_at(&self, now: Instant) -> String {
        let window = self
            .completed_at
            .saturating_duration_since(self.started_at)
            .as_nanos();
        let elapsed = now
            .saturating_duration_since(self.started_at)
            .as_nanos()
            .min(window);
        let percent = elapsed
            .saturating_mul(100)
            .checked_div(window)
            .and_then(|percent| u64::try_from(percent).ok())
            .unwrap_or(MAX_ACTIVE_IMPORT_PROGRESS)
            .min(MAX_ACTIVE_IMPORT_PROGRESS);
        percent.to_string()
    }

    /// This task as `ImportImage` reports it: [`Self::view_at`] through the members
    /// `ImportImageResult` declares, which are the task's own minus the `BootMode` that
    /// shape carries no member for.
    pub fn import_response(&self, now: Instant) -> ImportImageOutput {
        let view = self.view_at(now);
        ImportImageOutput {
            architecture: view.architecture,
            description: view.description,
            encrypted: view.encrypted,
            hypervisor: view.hypervisor,
            image_id: view.image_id,
            import_task_id: view.import_task_id,
            kms_key_id: view.kms_key_id,
            license_type: view.license_type,
            platform: view.platform,
            progress: view.progress,
            snapshot_details: view.snapshot_details,
            status: view.status,
            status_message: view.status_message,
            license_specifications: view.license_specifications,
            tags: view.tags,
            usage_operation: view.usage_operation,
        }
    }
}

/// The caller-chosen bytes one stored import image task holds: its description, the KMS
/// key and usage operation the request named, its tags, the license configuration ARNs,
/// the per-disk strings of its snapshot details, and the `ClientToken` the token map keeps
/// for it. Everything else on the record — the ids, the states, the architecture, the
/// platform and the license type — is derived here or drawn from a closed catalog, so it
/// is fixed-size.
///
/// Each ELEMENT of the two caller-chosen lists is charged
/// [`IMPORT_TASK_ELEMENT_OVERHEAD_BYTES`] beyond the strings it carries, so a request of
/// many empty-string disk containers cannot store a record per container for a zero-byte
/// charge — the same reason [`launch_template_payload_bytes`] charges the rendered size of
/// a nested caller structure rather than the string lengths inside it.
///
/// The ONE definition of that charge: `ImportImage` asks it before the write and charges
/// exactly it.
pub fn import_image_task_payload_bytes(view: &ImportImageTask, client_token: Option<&str>) -> usize {
    let snapshot_bytes: usize = view
        .snapshot_details
        .iter()
        .flatten()
        .map(|detail| {
            IMPORT_TASK_ELEMENT_OVERHEAD_BYTES
                + detail.description.as_ref().map_or(0, String::len)
                + detail.device_name.as_ref().map_or(0, String::len)
                + detail.url.as_ref().map_or(0, String::len)
                + detail.user_bucket.as_ref().map_or(0, |bucket| {
                    bucket.s3_bucket.as_ref().map_or(0, String::len)
                        + bucket.s3_key.as_ref().map_or(0, String::len)
                })
        })
        .sum();
    let license_bytes: usize = view
        .license_specifications
        .iter()
        .flatten()
        .map(|specification| {
            IMPORT_TASK_ELEMENT_OVERHEAD_BYTES
                + specification
                    .license_configuration_arn
                    .as_ref()
                    .map_or(0, String::len)
        })
        .sum();
    view.description
        .as_ref()
        .map_or(0, String::len)
        .saturating_add(view.kms_key_id.as_ref().map_or(0, String::len))
        .saturating_add(view.usage_operation.as_ref().map_or(0, String::len))
        .saturating_add(tag_list_bytes(view.tags.as_deref().unwrap_or_default()))
        .saturating_add(snapshot_bytes)
        .saturating_add(license_bytes)
        .saturating_add(client_token.map_or(0, str::len))
}

/// Whether `id` is a well-formed import image task id.
///
/// An import image task id is `import-ami-` followed by 8 or 17 lowercase hexadecimal
/// digits — the short and the long id form every EC2 resource id takes.
pub fn import_image_task_id_is_well_formed(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix(IMPORT_IMAGE_TASK_ID_PREFIX_WITH_HYPHEN) else {
        return false;
    };
    matches!(suffix.len(), 8 | 17)
        && suffix
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Refuse an `ImportTaskIds` entry that is not an import image task id at all: HTTP 400
/// [`INVALID_CONVERSION_TASK_ID`], EC2's code for a conversion task id it cannot resolve.
///
/// Only a MALFORMED id is refused. A well-formed id no task of the account has is not an
/// error: `DescribeImportImageTasks` answers "a list of zero or more import image tasks"
/// (`DescribeImportImageTasksResult$ImportImageTasks`), so such an id simply selects no
/// row — which is what the real service does with one.
pub fn require_well_formed_import_image_task_id<E>(
    task_id: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    if import_image_task_id_is_well_formed(task_id) {
        return Ok(());
    }
    Err(crate::scaffolded::Scaffolded::service_error(
        INVALID_CONVERSION_TASK_ID,
        400,
        format!("Invalid id: '{task_id}'"),
    ))
}

/// Refuse a create that would take the store past a ceiling of its own:
/// HTTP 400 [`RESOURCE_COUNT_EXCEEDED`].
pub fn resource_count_exceeded<E>(message: impl Into<String>) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(RESOURCE_COUNT_EXCEEDED, 400, message)
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another import image task fits inside the store's ceiling
    /// ([`MAX_STORED_IMPORT_IMAGE_TASKS`]).
    ///
    /// The same contract as [`Self::has_vpc_capacity`]: a create asks first and refuses
    /// instead of growing the store past its ceiling.
    pub fn has_import_image_task_capacity(&self) -> bool {
        self.import_image_tasks.len() < MAX_STORED_IMPORT_IMAGE_TASKS
    }

    /// Complete every import image task whose `active` -> `completed` move the clock owed
    /// by `now`.
    ///
    /// Reached only through [`Self::materialize_due_transitions`], so a reporting operation
    /// observes the completion without having caused it: the move was owed at this instant
    /// whether or not anybody called. It is a WRITE rather than a derive because completing
    /// an import MINTS resources — "the ID of the Amazon Machine Image (AMI) of the
    /// imported virtual machine" (`ImportImageTask$ImageId`) and "the snapshot ID of the
    /// disk being imported" (`SnapshotDetail$SnapshotId`) — which every later read has to
    /// agree on.
    pub fn complete_due_import_image_tasks(&mut self, now: Instant) {
        let due: Vec<String> = self
            .import_image_tasks
            .iter()
            .filter(|(_, record)| {
                record.view.status.as_deref() == Some(IMPORT_TASK_STATE_ACTIVE)
                    && now >= record.completed_at
            })
            .map(|(task_id, _)| task_id.clone())
            .collect();
        for task_id in due {
            // The AMI the import creates is an image of this store like any other, so it is
            // stored through the one writer of an image rather than beside it.
            let Some(spec) = self.imported_image_spec(&task_id) else {
                continue;
            };
            // No room for the AMI means the import cannot produce one at all, so it ends in
            // the state a failed import ends in rather than staying `active` for ever: the
            // task is `deleted` and its message says which ceiling stopped it.
            if !self.has_image_capacity() {
                if let Some(record) = self.import_image_tasks.get_mut(&task_id) {
                    record.view.status = Some(IMPORT_TASK_STATE_DELETED.to_string());
                    record.view.status_message = Some(format!(
                        "This emulator holds {MAX_IMAGES} images, which is as many as it \
                         stores, so the imported AMI could not be created"
                    ));
                    record.view.progress = None;
                    for detail in record.view.snapshot_details.iter_mut().flatten() {
                        detail.status = Some(IMPORT_TASK_STATE_DELETED.to_string());
                        detail.progress = None;
                    }
                }
                continue;
            }
            let image_id = self.insert_image(spec);
            let disks = self
                .import_image_tasks
                .get(&task_id)
                .map_or(0, |record| {
                    record.view.snapshot_details.as_ref().map_or(0, Vec::len)
                });
            let snapshot_ids: Vec<String> = (0..disks).map(|_| self.next_id("snap")).collect();
            if let Some(record) = self.import_image_tasks.get_mut(&task_id) {
                record.view.status = Some(IMPORT_TASK_STATE_COMPLETED.to_string());
                // The conversion is over, so there is no stage left to report and no
                // percentage left to make.
                record.view.status_message = None;
                record.view.progress = None;
                record.view.image_id = Some(image_id);
                for (detail, snapshot_id) in record
                    .view
                    .snapshot_details
                    .iter_mut()
                    .flatten()
                    .zip(snapshot_ids)
                {
                    detail.snapshot_id = Some(snapshot_id);
                    detail.status = Some(IMPORT_TASK_STATE_COMPLETED.to_string());
                    detail.progress = None;
                }
            }
        }
    }

    /// The AMI a completed import image task created, as [`Self::insert_image`] takes it.
    ///
    /// Everything comes off the task: the AMI is named after the import that created it,
    /// carries the architecture, platform and boot mode the request named, and its root
    /// volume is as large as the largest disk the import converted. The task's own tags are
    /// NOT copied onto it — "the tags to apply to the import image task during creation"
    /// (`ImportImageRequest$TagSpecifications`) tags the task, not the image.
    fn imported_image_spec(&self, task_id: &str) -> Option<ImageSpec> {
        let record = self.import_image_tasks.get(task_id)?;
        let windows = record.view.platform.as_deref() == Some(IMAGE_PLATFORM_WINDOWS);
        let defaults = ImageSpec::default();
        let default_architecture = defaults.architecture.clone();
        let root_volume_size = record
            .view
            .snapshot_details
            .iter()
            .flatten()
            .filter_map(|detail| detail.disk_image_size)
            .filter(|size| size.is_finite() && *size >= 1.0)
            .map(|size| i32::try_from(size.ceil() as i64).unwrap_or(i32::MAX))
            .max()
            .unwrap_or(if windows {
                WINDOWS_ROOT_VOLUME_SIZE
            } else {
                LINUX_ROOT_VOLUME_SIZE
            });
        Some(ImageSpec {
            name: task_id.to_string(),
            description: record.view.description.clone().unwrap_or_default(),
            architecture: record
                .view
                .architecture
                .clone()
                .unwrap_or(default_architecture),
            platform: record.view.platform.clone().filter(|_| windows),
            platform_details: if windows {
                PLATFORM_DETAILS_WINDOWS.to_string()
            } else {
                PLATFORM_DETAILS_LINUX.to_string()
            },
            usage_operation: record.view.usage_operation.clone().unwrap_or_else(|| {
                if windows {
                    USAGE_OPERATION_WINDOWS.to_string()
                } else {
                    USAGE_OPERATION_LINUX.to_string()
                }
            }),
            boot_mode: record.view.boot_mode.clone(),
            root_device_name: if windows {
                WINDOWS_ROOT_DEVICE_NAME.to_string()
            } else {
                LINUX_ROOT_DEVICE_NAME.to_string()
            },
            root_volume_size,
            state: IMAGE_STATE_AVAILABLE.to_string(),
            ..defaults
        })
    }
}

/// Refuse a request that named a snapshot id no snapshot of the account has, the way
/// EC2 does: HTTP 400 `InvalidSnapshot.NotFound`.
pub fn invalid_snapshot_id_not_found<E>(snapshot_id: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_SNAPSHOT_ID_NOT_FOUND,
        400,
        format!("The snapshot '{snapshot_id}' does not exist."),
    )
}

/// Whether `id` is a well-formed EBS snapshot id: `snap-` followed by 8 or 17 lowercase
/// hexadecimal digits — the short and the long id form, as the model's own
/// `^snap-([0-9a-f]{8}|[0-9a-f]{17})$` style id patterns read.
pub fn snapshot_id_is_well_formed(id: &str) -> bool {
    prefixed_id_is_well_formed("snap", id)
}

/// Refuse a snapshot id the request named that this service cannot resolve: a malformed
/// one is answered as malformed, and a well-formed one as not found — the way the
/// service orders the two, and the same ordering [`require_known_volume`] keeps.
pub fn unresolvable_snapshot_id<E>(snapshot_id: &str) -> crate::scaffolded::Scaffolded<E> {
    if snapshot_id_is_well_formed(snapshot_id) {
        return invalid_snapshot_id_not_found(snapshot_id);
    }
    crate::scaffolded::Scaffolded::service_error(
        INVALID_SNAPSHOT_ID_MALFORMED,
        400,
        format!("Invalid id: '{snapshot_id}'"),
    )
}

/// Refuse a create that would take the store past [`MAX_STORED_VOLUMES`]: HTTP 400
/// `VolumeLimitExceeded`, EC2's own volumes-per-Region quota code.
pub fn volume_limit_exceeded<E>(message: impl Into<String>) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(VOLUME_LIMIT_EXCEEDED, 400, message)
}

// =============================================================================
// EBS volume modifications
// =============================================================================

/// A modification that has been accepted and whose change is being applied:
/// `VolumeModificationState`'s `modifying`
/// (`smithy_query(query="shape",shape="VolumeModificationState")`).
pub const VOLUME_MODIFICATION_STATE_MODIFYING: &str = "modifying";

/// A modification whose change is in place and whose volume is being brought to its new
/// performance: `VolumeModificationState`'s `optimizing`.
pub const VOLUME_MODIFICATION_STATE_OPTIMIZING: &str = "optimizing";

/// A modification that is finished: `VolumeModificationState`'s `completed`. The fourth
/// value of the catalog, `failed`, is never stored here — nothing in this emulator fails a
/// modification after it has been accepted; a change it cannot apply is refused at the
/// call that asks for it.
pub const VOLUME_MODIFICATION_STATE_COMPLETED: &str = "completed";

/// The `Progress` a finished modification reports: "The modification progress, from 0 to
/// 100 percent complete" (`VolumeModification$Progress`).
pub const VOLUME_MODIFICATION_PROGRESS_COMPLETE: i64 = 100;

/// One EBS volume's most recent modification: the modelled [`VolumeModification`] every
/// operation that reports one answers with, plus the instants the clock owes it its
/// `modifying` -> `optimizing` -> `completed` moves.
///
/// The view holds no `ModificationState`, `Progress` or `EndTime`: all three are facts of
/// the modification's age, so [`VolumeModificationRecord::view_at`] derives them rather
/// than letting two writers store two different answers — the arrangement
/// [`VolumeRecord`] uses for a volume's own `State`.
#[derive(Debug, Clone)]
pub struct VolumeModificationRecord {
    /// The modification as the model shapes it: the volume it belongs to, its original
    /// values, its target values and its `StartTime`. `ModificationState`, `Progress` and
    /// `EndTime` are left for [`Self::view_at`] to fill.
    pub view: VolumeModification,
    /// The instant the modification started, which [`Self::progress_at`] measures from.
    pub started_at: Instant,
    /// The instant from which this modification's `modifying` -> `optimizing` move is
    /// due. Monotonic ([`Instant`]), because it is only ever compared against the clock
    /// and never reported, so a wall-clock adjustment cannot move a finished
    /// modification back to `modifying`.
    pub optimizing_at: Instant,
    /// The instant from which its `optimizing` -> `completed` move is due.
    pub completed_at: Instant,
    /// The wall-clock instant the modification finishes on, which it reports as its
    /// `EndTime` once it has: the modification ended when that move fell due, not when
    /// somebody next asked about it.
    pub ends_at: SystemTime,
}

impl VolumeModificationRecord {
    /// A modification of `view` that starts now: `modifying`, due `optimizing` one
    /// [`TRANSITION_DELAY`] later and `completed` one further [`TRANSITION_DELAY`] after
    /// that.
    ///
    /// Each move falls due a span after the move before it, read off the same monotonic
    /// clock as every other clock-owed transition in this crate and recorded rather than
    /// slept for, so a caller that reads straight after the call that started the
    /// modification still observes it `modifying`.
    pub fn starting_now(view: VolumeModification) -> Self {
        let started_at = Instant::now();
        Self {
            view,
            started_at,
            optimizing_at: started_at + TRANSITION_DELAY,
            completed_at: started_at + TRANSITION_DELAY * 2,
            ends_at: SystemTime::now() + TRANSITION_DELAY * 2,
        }
    }

    /// The modification's `VolumeModificationState` at `now`.
    pub fn state_at(&self, now: Instant) -> &'static str {
        if now < self.optimizing_at {
            VOLUME_MODIFICATION_STATE_MODIFYING
        } else if now < self.completed_at {
            VOLUME_MODIFICATION_STATE_OPTIMIZING
        } else {
            VOLUME_MODIFICATION_STATE_COMPLETED
        }
    }

    /// How far along the modification is at `now`, "from 0 to 100 percent complete"
    /// (`VolumeModification$Progress`): the share of the span between its start and its
    /// completion that the clock has passed, and [`VOLUME_MODIFICATION_PROGRESS_COMPLETE`]
    /// once it is `completed`.
    pub fn progress_at(&self, now: Instant) -> i64 {
        let whole = self.completed_at.saturating_duration_since(self.started_at);
        let elapsed = now.saturating_duration_since(self.started_at);
        if whole.is_zero() || elapsed >= whole {
            return VOLUME_MODIFICATION_PROGRESS_COMPLETE;
        }
        let percent = elapsed
            .as_nanos()
            .saturating_mul(u128::try_from(VOLUME_MODIFICATION_PROGRESS_COMPLETE).unwrap_or(100))
            / whole.as_nanos();
        i64::try_from(percent)
            .unwrap_or(VOLUME_MODIFICATION_PROGRESS_COMPLETE)
            .clamp(0, VOLUME_MODIFICATION_PROGRESS_COMPLETE)
    }

    /// This modification as of `now`: the stored view with the state, the progress and the
    /// end time the clock has reached. A read observes the moves the clock already owed
    /// without writing anything, so it cannot change what a later caller sees.
    ///
    /// `EndTime` is "the modification completion or failure time"
    /// (`VolumeModification$EndTime`), so a modification that has not finished reports
    /// none at all.
    pub fn view_at(&self, now: Instant) -> VolumeModification {
        let mut view = self.view.clone();
        view.modification_state = Some(self.state_at(now).to_string());
        view.progress = Some(self.progress_at(now));
        view.end_time = (now >= self.completed_at).then(|| system_time_to_date_time(self.ends_at));
        view
    }
}

/// Refuse a request that named an EBS volume this account holds no modification for, the
/// way EC2 does: HTTP 400 `InvalidVolumeModification.NotFound`, whose message names the
/// volume the request asked about — the one thing about the refusal that is not fixed text.
pub fn invalid_volume_modification_not_found<E>(
    volume_id: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_VOLUME_MODIFICATION_NOT_FOUND,
        400,
        format!("Modification for volume '{volume_id}' does not exist."),
    )
}

// =============================================================================
// Secondary networks
// =============================================================================

/// The prefix of a secondary network's id.
///
/// `com.amazonaws.ec2#SecondaryNetworkId` carries no `@pattern`, so the model publishes no
/// spelling for it; this service mints EC2's own `<prefix>-<17 lowercase hex digits>` id
/// form ([`ComAmazonawsEc2Amazonec2State::next_id`]) with a prefix naming the resource, the
/// way every other id of this store is minted.
pub const SECONDARY_NETWORK_ID_PREFIX: &str = "sn";

/// The prefix of the id of a secondary network's IPv4 CIDR block association
/// (`com.amazonaws.ec2#SecondaryNetworkCidrAssociationId`, which publishes no pattern
/// either), minted the same way as [`SECONDARY_NETWORK_ID_PREFIX`].
pub const SECONDARY_NETWORK_CIDR_ASSOCIATION_ID_PREFIX: &str = "sn-cidr-assoc";

/// A secondary network AWS has not finished creating: `SecondaryNetworkState`'s
/// `create-in-progress` (`smithy_query(query="shape",shape="SecondaryNetworkState")`).
pub const SECONDARY_NETWORK_STATE_CREATE_IN_PROGRESS: &str = "create-in-progress";

/// A secondary network AWS has finished creating: `SecondaryNetworkState`'s
/// `create-complete`.
pub const SECONDARY_NETWORK_STATE_CREATE_COMPLETE: &str = "create-complete";

/// A secondary network AWS has been asked to delete and has not finished deleting:
/// `SecondaryNetworkState`'s `delete-in-progress`.
pub const SECONDARY_NETWORK_STATE_DELETE_IN_PROGRESS: &str = "delete-in-progress";

/// A secondary network AWS has finished deleting: `SecondaryNetworkState`'s
/// `delete-complete`.
///
/// A network in either delete state is still REPORTED: `DescribeSecondaryNetworks`
/// documents `delete-in-progress` and `delete-complete` among the values of its `state`
/// filter (`smithy_query(query="doc",shape="DescribeSecondaryNetworksRequest",member="Filters")`),
/// which no report could ever select on if a deleted network left the account's networks.
pub const SECONDARY_NETWORK_STATE_DELETE_COMPLETE: &str = "delete-complete";

/// The IPv4 CIDR block of a secondary network that is being deleted:
/// `SecondaryNetworkCidrBlockAssociationState`'s `disassociating`.
pub const SECONDARY_NETWORK_CIDR_STATE_DISASSOCIATING: &str = "disassociating";

/// The IPv4 CIDR block of a secondary network that has finished being deleted:
/// `SecondaryNetworkCidrBlockAssociationState`'s `disassociated`.
pub const SECONDARY_NETWORK_CIDR_STATE_DISASSOCIATED: &str = "disassociated";

/// An IPv4 CIDR block whose association with a secondary network is still being made:
/// `SecondaryNetworkCidrBlockAssociationState`'s `associating`.
pub const SECONDARY_NETWORK_CIDR_STATE_ASSOCIATING: &str = "associating";

/// An IPv4 CIDR block associated with a secondary network:
/// `SecondaryNetworkCidrBlockAssociationState`'s `associated`.
pub const SECONDARY_NETWORK_CIDR_STATE_ASSOCIATED: &str = "associated";

/// The one resource type `CreateSecondaryNetwork`'s `TagSpecifications` may name: "The tags
/// to assign to the secondary network"
/// (`CreateSecondaryNetworkRequest$TagSpecifications`), spelled as the modelled
/// `ResourceType` enum spells it.
pub const SECONDARY_NETWORK_TAGGABLE_RESOURCE: &str = "secondary-network";

/// The largest number of secondary networks this in-memory store holds.
///
/// A capacity bound in the style of [`MAX_VPCS`], not an AWS quota: each network is a
/// record in this process's memory, so the number a caller can create has to be finite.
/// A count ceiling is not a memory bound on its own — see [`MAX_STORED_BYTES`] — so the
/// caller-chosen bytes of each network are charged against that budget too
/// ([`secondary_network_payload_bytes`]).
pub const MAX_SECONDARY_NETWORKS: usize = 1_000;

/// The error code a create past [`MAX_SECONDARY_NETWORKS`] is refused with.
///
/// AWS publishes no quota code for this resource, so this follows EC2's own
/// `<Resource>LimitExceeded` spelling ([`VPC_LIMIT_EXCEEDED`], [`SUBNET_LIMIT_EXCEEDED`],
/// [`SECURITY_GROUP_LIMIT_EXCEEDED`]).
pub const SECONDARY_NETWORK_LIMIT_EXCEEDED: &str = "SecondaryNetworkLimitExceeded";

/// The error code for a request naming a secondary network id the account holds no network
/// for, in EC2's `Invalid<Resource>Id.NotFound` spelling — the same spelling as
/// [`INVALID_LAUNCH_TEMPLATE_ID_NOT_FOUND`].
pub const INVALID_SECONDARY_NETWORK_ID_NOT_FOUND: &str = "InvalidSecondaryNetworkId.NotFound";

/// One stored secondary network: the modelled [`SecondaryNetwork`] every operation that
/// reports one answers with, plus the instant the clock owes it its `create-in-progress`
/// -> `create-complete` move.
///
/// The view holds no `State` — neither the network's own nor that of its IPv4 CIDR block
/// associations: both are facts of the network's age, so [`SecondaryNetworkRecord::view_at`]
/// derives them rather than letting two writers store two different words for one network.
#[derive(Debug, Clone)]
pub struct SecondaryNetworkRecord {
    /// The network as the model shapes it, with the `State` members left for
    /// [`SecondaryNetworkRecord::view_at`] to fill.
    pub view: SecondaryNetwork,
    /// The instant from which this network's `create-in-progress` -> `create-complete`
    /// move is due. Monotonic ([`Instant`]), because it is only ever compared against the
    /// clock and never reported, so a wall-clock adjustment cannot move a network back to
    /// `create-in-progress`.
    pub create_complete_at: Instant,
    /// The instant from which this network's `delete-in-progress` -> `delete-complete`
    /// move is due, and `None` for a network nobody has asked to delete. Set once, by the
    /// delete that asked for it ([`ComAmazonawsEc2Amazonec2State::begin_secondary_network_deletion`]),
    /// and monotonic for the same reason as [`Self::create_complete_at`].
    pub delete_complete_at: Option<Instant>,
}

impl SecondaryNetworkRecord {
    /// This network as of `now`: the stored view with the states the clock has reached. A
    /// read observes the transition the clock already owed without writing anything.
    pub fn view_at(&self, now: Instant) -> SecondaryNetwork {
        let mut view = self.view.clone();
        view.state = Some(self.state_at(now).to_string());
        let association_state = self.cidr_association_state_at(now);
        for association in view.ipv4_cidr_block_associations.iter_mut().flatten() {
            association.state = Some(association_state.to_string());
        }
        view
    }

    /// The network's `SecondaryNetworkState` at `now`.
    ///
    /// A deletion that has been asked for is the only move left, so it takes precedence:
    /// a network asked to delete while it was still being created does not report
    /// `create-complete` once the creation's own move falls due.
    pub fn state_at(&self, now: Instant) -> &'static str {
        if let Some(delete_complete_at) = self.delete_complete_at {
            return if now < delete_complete_at {
                SECONDARY_NETWORK_STATE_DELETE_IN_PROGRESS
            } else {
                SECONDARY_NETWORK_STATE_DELETE_COMPLETE
            };
        }
        if now < self.create_complete_at {
            SECONDARY_NETWORK_STATE_CREATE_IN_PROGRESS
        } else {
            SECONDARY_NETWORK_STATE_CREATE_COMPLETE
        }
    }

    /// The `SecondaryNetworkCidrBlockAssociationState` of this network's IPv4 CIDR block
    /// associations at `now`: a block is associated once the network it belongs to has
    /// finished being created and disassociated once it has finished being deleted, so the
    /// block's moves fall due with the network's own.
    pub fn cidr_association_state_at(&self, now: Instant) -> &'static str {
        if let Some(delete_complete_at) = self.delete_complete_at {
            return if now < delete_complete_at {
                SECONDARY_NETWORK_CIDR_STATE_DISASSOCIATING
            } else {
                SECONDARY_NETWORK_CIDR_STATE_DISASSOCIATED
            };
        }
        if now < self.create_complete_at {
            SECONDARY_NETWORK_CIDR_STATE_ASSOCIATING
        } else {
            SECONDARY_NETWORK_CIDR_STATE_ASSOCIATED
        }
    }
}

/// What one secondary network is created with: everything a writer chooses, with every id
/// and every derived member left to [`ComAmazonawsEc2Amazonec2State::insert_secondary_network`].
#[derive(Debug, Clone)]
pub struct SecondaryNetworkSpec {
    /// The network's IPv4 CIDR block, already canonical
    /// ([`canonical_secondary_network_ipv4_cidr`]).
    pub ipv4_cidr_block: String,
    /// The `SecondaryNetworkType` the request named.
    pub network_type: String,
    /// The Region the request was signed for, which the network's ARN names.
    pub region: String,
    /// The tags the request assigned to the network.
    pub tags: Vec<Tag>,
    /// When this network's `create-in-progress` -> `create-complete` move falls due.
    pub create_complete_at: Instant,
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another secondary network fits inside the store's ceiling
    /// ([`MAX_SECONDARY_NETWORKS`]).
    ///
    /// The same contract as [`Self::has_vpc_capacity`]: a create asks first and refuses
    /// with [`SECONDARY_NETWORK_LIMIT_EXCEEDED`] when it answers `false`.
    pub fn has_secondary_network_capacity(&self) -> bool {
        self.secondary_networks.len() < MAX_SECONDARY_NETWORKS
    }

    /// Insert a secondary network, and answer the `SecondaryNetworkId` it was given.
    ///
    /// Everything the service itself owns is derived here rather than taken from the
    /// caller — the network's id, its ARN, the association id of its IPv4 CIDR block and
    /// the owning account — so two writers cannot report the same network differently.
    ///
    /// Like [`Self::insert_vpc`], the caller-chosen bytes of `spec.tags` are charged
    /// against the store's byte budget here, so a writer cannot store them uncharged; a
    /// create still asks [`Self::has_secondary_network_capacity`] and
    /// [`Self::would_exceed_stored_bytes`] first so it can refuse instead of overshooting.
    pub fn insert_secondary_network(&mut self, spec: SecondaryNetworkSpec) -> String {
        let secondary_network_id = self.next_id(SECONDARY_NETWORK_ID_PREFIX);
        let association_id = self.next_id(SECONDARY_NETWORK_CIDR_ASSOCIATION_ID_PREFIX);
        let view = SecondaryNetwork {
            secondary_network_id: Some(secondary_network_id.clone()),
            secondary_network_arn: Some(::engine::emulator_arn(
                "ec2",
                &spec.region,
                &format!("{SECONDARY_NETWORK_TAGGABLE_RESOURCE}/{secondary_network_id}"),
            )),
            owner_id: Some(::engine::emulator_account_id().to_string()),
            r#type: Some(spec.network_type),
            // Both states are derived from `create_complete_at` on every read.
            state: None,
            // "The reason for the current state" (`SecondaryNetwork$StateReason`): a
            // network that is being created as asked has nothing to explain, and no
            // operation of this emulator fails one.
            state_reason: None,
            ipv4_cidr_block_associations: Some(vec![SecondaryNetworkIpv4CidrBlockAssociation {
                association_id: Some(association_id),
                cidr_block: Some(spec.ipv4_cidr_block),
                state: None,
                state_reason: None,
            }]),
            tags: Some(spec.tags),
        };
        self.add_stored_bytes(tag_list_bytes(view.tags.as_deref().unwrap_or_default()));
        self.secondary_networks.insert(
            secondary_network_id.clone(),
            SecondaryNetworkRecord {
                view,
                create_complete_at: spec.create_complete_at,
                // Nobody has asked to delete a network that is being created.
                delete_complete_at: None,
            },
        );
        secondary_network_id
    }

    /// Start deleting the secondary network `secondary_network_id`, and answer it as the
    /// store then holds it — `None` when the account holds no such network.
    ///
    /// The deletion is a CLOCK-OWED move, like the creation: the network is
    /// `delete-in-progress` with its IPv4 CIDR block `disassociating` until
    /// `delete_complete_at`, which is stored rather than slept for, and
    /// [`SecondaryNetworkRecord::view_at`] derives both states from it on every read.
    ///
    /// The record STAYS in the store, because a network in either delete state is still
    /// reported ([`SECONDARY_NETWORK_STATE_DELETE_COMPLETE`]) — so its caller-chosen bytes
    /// stay charged to the store's budget, which still holds them, and it still counts
    /// against [`MAX_SECONDARY_NETWORKS`], which bounds the records this process holds.
    ///
    /// A network already being deleted keeps the due instant its own delete gave it: a
    /// repeat of the call reports the deletion already under way instead of restarting its
    /// clock.
    pub fn begin_secondary_network_deletion(
        &mut self,
        secondary_network_id: &str,
        now: Instant,
        delete_complete_at: Instant,
    ) -> Option<SecondaryNetwork> {
        let record = self.secondary_networks.get_mut(secondary_network_id)?;
        record.delete_complete_at.get_or_insert(delete_complete_at);
        Some(record.view_at(now))
    }
}

/// The caller-chosen bytes one stored secondary network holds: its tags and the
/// `ClientToken` the token map keeps for it. Everything else on the record — the ids, the
/// ARN, the account, the canonical CIDR block and the states — is derived by this module or
/// bounded by the model, so it is fixed-size.
///
/// The ONE definition of that charge: `CreateSecondaryNetwork` asks it before the write and
/// charges exactly it, and a removal path must give back the same number.
pub fn secondary_network_payload_bytes(tags: &[Tag], client_token: Option<&str>) -> usize {
    tag_list_bytes(tags).saturating_add(client_token.map_or(0, str::len))
}

/// Refuse a secondary network id the request named that this service cannot resolve.
///
/// `SecondaryNetworkId` publishes no pattern, so there is no spelling to call an id
/// malformed by: an id no network of the account has is answered as not found whatever it
/// looks like, rather than being sorted into two codes on a rule AWS does not state.
pub fn require_known_secondary_network<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    secondary_network_id: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    if store.secondary_networks.contains_key(secondary_network_id) {
        return Ok(());
    }
    Err(invalid_secondary_network_id_not_found(secondary_network_id))
}

/// Refuse a request that named a secondary network id no network of the account has: HTTP
/// 400 [`INVALID_SECONDARY_NETWORK_ID_NOT_FOUND`].
pub fn invalid_secondary_network_id_not_found<E>(
    secondary_network_id: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_SECONDARY_NETWORK_ID_NOT_FOUND,
        400,
        format!("The secondary network '{secondary_network_id}' does not exist."),
    )
}

/// Refuse a create that would take the store past [`MAX_SECONDARY_NETWORKS`]: HTTP 400
/// [`SECONDARY_NETWORK_LIMIT_EXCEEDED`].
pub fn secondary_network_limit_exceeded<E>(
    message: impl Into<String>,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(SECONDARY_NETWORK_LIMIT_EXCEEDED, 400, message)
}

/// The Region this request was signed for, from its SigV4 credential scope; the
/// emulator's default Region when it carries none.
///
/// The scope is caller-controlled text that is rendered into zone names and zone ids,
/// so a value that is not shaped like a Region name is refused here and the default
/// used instead.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 64
                && region
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsEc2Amazonec2State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsEc2Amazonec2State>)
}

// =============================================================================
// Verified Access trust providers
// =============================================================================

/// The id prefix a Verified Access trust provider is minted under.
///
/// EC2 names the resource's ids `vatp-<hex>`, the same `<prefix>-<hex>` form every other
/// EC2 resource id takes, so [`ComAmazonawsEc2Amazonec2State::next_id`] mints them.
pub const VERIFIED_ACCESS_TRUST_PROVIDER_ID_PREFIX: &str = "vatp";

/// The one `ResourceType` a `CreateVerifiedAccessTrustProvider` tag specification may
/// name: "The tags to assign to the Verified Access trust provider"
/// (`CreateVerifiedAccessTrustProviderRequest$TagSpecifications`), spelled as the
/// modelled `ResourceType` enum spells it
/// (`smithy_query(query="shape",shape="ResourceType")`).
pub const VERIFIED_ACCESS_TRUST_PROVIDER_TAGGABLE_RESOURCE: &str =
    "verified-access-trust-provider";

/// `TrustProviderType`'s `user` (`smithy_query(query="shape",shape="TrustProviderType")`).
pub const TRUST_PROVIDER_TYPE_USER: &str = "user";

/// `TrustProviderType`'s `device`.
pub const TRUST_PROVIDER_TYPE_DEVICE: &str = "device";

/// `UserTrustProviderType`'s `oidc`
/// (`smithy_query(query="shape",shape="UserTrustProviderType")`) — the one user-based
/// kind that is OpenID Connect, so the one `OidcOptions` describe.
pub const USER_TRUST_PROVIDER_TYPE_OIDC: &str = "oidc";

/// How many Verified Access trust providers this emulator stores before
/// `CreateVerifiedAccessTrustProvider` is refused.
///
/// CHOSEN, like [`MAX_STORED_VOLUMES`] and for the same reason: nothing in this emulator
/// reclaims a stored trust provider, so the store needs a ceiling that is not the
/// process's memory. It is well above the published per-Region Verified Access quota a
/// test would exercise. A count ceiling is not a memory bound on its own — see
/// [`MAX_STORED_BYTES`] — so the caller-chosen bytes of each record are charged against
/// that budget too ([`verified_access_trust_provider_payload_bytes`]).
pub const MAX_VERIFIED_ACCESS_TRUST_PROVIDERS: usize = 1_000;

/// The largest page `DescribeVerifiedAccessTrustProviders` serves, and the page it serves
/// when the request names no size.
///
/// The top of the modelled `MaxResults` range, `range 5..200`
/// (`smithy_query(query="operation",operation="DescribeVerifiedAccessTrustProviders")`),
/// so a request that names no page size is answered with the largest page the operation
/// can be ASKED for and the `NextToken` path behaves the same either way.
pub const MAX_VERIFIED_ACCESS_TRUST_PROVIDERS_PAGE_SIZE: usize = 200;

/// EC2's code for a Verified Access trust provider id that names no trust provider of the
/// account.
///
/// EC2's model declares no error shape at all (see [`crate::scaffolded::ServiceError`]),
/// and the service answers `Invalid<Resource>Id.NotFound` for an id of a resource it
/// cannot resolve — `InvalidVolume.NotFound`, `InvalidLaunchTemplateId.NotFound` — so
/// that is the code this resource's id gets.
pub const INVALID_VERIFIED_ACCESS_TRUST_PROVIDER_ID_NOT_FOUND: &str =
    "InvalidVerifiedAccessTrustProviderId.NotFound";

/// EC2's quota code for one Verified Access trust provider too many, named the way every
/// other per-resource EC2 quota code is (`VolumeLimitExceeded`,
/// `LaunchTemplateLimitExceeded`).
pub const VERIFIED_ACCESS_TRUST_PROVIDER_LIMIT_EXCEEDED: &str =
    "VerifiedAccessTrustProviderLimitExceeded";

/// The caller-chosen bytes one stored trust provider holds for the process's lifetime:
/// every string the REQUEST chose the length of, plus the client token the token map
/// keeps.
///
/// The ids, the timestamps and the trust-provider kinds are the service's own or come from
/// a modelled enum, so they are bounded already and not counted here. One definition of
/// the charge, so a writer cannot store bytes the store's budget never saw.
pub fn verified_access_trust_provider_payload_bytes(
    provider: &crate::ops::types::VerifiedAccessTrustProvider,
    client_token: Option<&str>,
) -> usize {
    let text = |value: &Option<String>| value.as_ref().map_or(0, String::len);
    let mut bytes = tag_list_bytes(provider.tags.as_deref().unwrap_or_default())
        .saturating_add(text(&provider.description))
        .saturating_add(text(&provider.policy_reference_name))
        .saturating_add(client_token.map_or(0, str::len));
    if let Some(options) = provider.oidc_options.as_ref() {
        bytes = bytes
            .saturating_add(text(&options.issuer))
            .saturating_add(text(&options.authorization_endpoint))
            .saturating_add(text(&options.token_endpoint))
            .saturating_add(text(&options.user_info_endpoint))
            .saturating_add(text(&options.client_id))
            .saturating_add(text(&options.client_secret))
            .saturating_add(text(&options.scope));
    }
    if let Some(options) = provider.device_options.as_ref() {
        bytes = bytes
            .saturating_add(text(&options.tenant_id))
            .saturating_add(text(&options.public_signing_key_url));
    }
    if let Some(options) = provider.native_application_oidc_options.as_ref() {
        bytes = bytes
            .saturating_add(text(&options.public_signing_key_endpoint))
            .saturating_add(text(&options.issuer))
            .saturating_add(text(&options.authorization_endpoint))
            .saturating_add(text(&options.token_endpoint))
            .saturating_add(text(&options.user_info_endpoint))
            .saturating_add(text(&options.client_id))
            .saturating_add(text(&options.scope));
    }
    if let Some(sse) = provider.sse_specification.as_ref() {
        bytes = bytes.saturating_add(text(&sse.kms_key_arn));
    }
    bytes
}

/// Refuse a request that named a Verified Access trust provider id this account holds no
/// trust provider for: HTTP 400
/// [`INVALID_VERIFIED_ACCESS_TRUST_PROVIDER_ID_NOT_FOUND`].
///
/// One code covers an id of any shape: an id this service never minted names no trust
/// provider of the account whether or not it reads like one. The message names the id the
/// REQUEST gave, in the wording the real service answers this resource with
/// ("VerifiedAccessTrustProvider <id> does not exist"), so a client reading the text sees
/// which id it was.
pub fn invalid_verified_access_trust_provider_id_not_found<E>(
    trust_provider_id: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_VERIFIED_ACCESS_TRUST_PROVIDER_ID_NOT_FOUND,
        400,
        format!("VerifiedAccessTrustProvider {trust_provider_id} does not exist"),
    )
}

/// Refuse a create that would take the store past
/// [`MAX_VERIFIED_ACCESS_TRUST_PROVIDERS`]: HTTP 400
/// [`VERIFIED_ACCESS_TRUST_PROVIDER_LIMIT_EXCEEDED`].
pub fn verified_access_trust_provider_limit_exceeded<E>(
    message: impl Into<String>,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        VERIFIED_ACCESS_TRUST_PROVIDER_LIMIT_EXCEEDED,
        400,
        message,
    )
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another Verified Access trust provider fits inside the store's ceiling
    /// ([`MAX_VERIFIED_ACCESS_TRUST_PROVIDERS`]).
    ///
    /// The same contract as [`Self::has_vpc_capacity`]: a create asks first and refuses
    /// with [`verified_access_trust_provider_limit_exceeded`] when it answers `false`.
    pub fn has_verified_access_trust_provider_capacity(&self) -> bool {
        self.verified_access_trust_providers.len() < MAX_VERIFIED_ACCESS_TRUST_PROVIDERS
    }

    /// Store `provider` under a freshly minted `VerifiedAccessTrustProviderId`, charge the
    /// caller-chosen bytes it and `client_token` retain against the store's byte budget,
    /// and answer the id it was given.
    ///
    /// The id is minted HERE rather than by the writer, so a trust provider never shares
    /// an id with another resource of this store, and the record is written with that id on
    /// it so no reader has to learn the key twice. The charge happens at the insert the way
    /// [`Self::insert_vpc`] charges a VPC's tags, so no path can store those bytes
    /// uncharged; a create still asks
    /// [`Self::has_verified_access_trust_provider_capacity`] and
    /// [`Self::would_exceed_stored_bytes`] first so it can refuse instead of overshooting.
    pub fn insert_verified_access_trust_provider(
        &mut self,
        mut provider: crate::ops::types::VerifiedAccessTrustProvider,
        client_token: Option<&str>,
    ) -> String {
        let trust_provider_id = self.next_id(VERIFIED_ACCESS_TRUST_PROVIDER_ID_PREFIX);
        provider.verified_access_trust_provider_id = Some(trust_provider_id.clone());
        self.add_stored_bytes(verified_access_trust_provider_payload_bytes(
            &provider,
            client_token,
        ));
        if let Some(token) = client_token {
            self.verified_access_trust_provider_client_tokens
                .insert(token.to_string(), trust_provider_id.clone());
        }
        self.verified_access_trust_providers
            .insert(trust_provider_id.clone(), provider);
        trust_provider_id
    }
}

// ---------------------------------------------------------------------------
// Modelled values and bounds the VPC create path uses
// ---------------------------------------------------------------------------

/// A VPC AWS is still creating: the `VpcState` enum's `pending` member, which is what
/// `CreateVpc` answers with until [`TRANSITION_DELAY`] has passed.
pub const VPC_STATE_PENDING: &str = "pending";

/// An IPv6 CIDR block still being associated with its VPC: the `VpcCidrBlockStateCode`
/// enum's `associating` member.
pub const CIDR_BLOCK_STATE_ASSOCIATING: &str = "associating";

/// The tenancy a VPC may be created with beside [`DEFAULT_INSTANCE_TENANCY`]: the
/// `Tenancy` enum's `dedicated` member. Its third member, `host`, is excluded by
/// `CreateVpcRequest$InstanceTenancy`'s own documentation.
pub const TENANCY_DEDICATED: &str = "dedicated";

/// The `ResourceType` a `CreateVpc` tag specification may name: the VPC it creates.
pub const RESOURCE_TYPE_VPC: &str = "vpc";

// ---------------------------------------------------------------------------
// The attributes of a VPC (`VpcAttributeName`)
// ---------------------------------------------------------------------------

/// The `VpcAttributeName` enum's `enableDnsHostnames` member.
///
/// Named here so the handlers and [`VpcAttributes`] spell one attribute one way; the
/// generated catalog of every member is `crate::ops::types::VPC_ATTRIBUTE_NAME_VALUES`,
/// which `DescribeVpcAttribute` validates its `Attribute` against and the crate's tests
/// pin these three constants to.
pub const VPC_ATTRIBUTE_ENABLE_DNS_HOSTNAMES: &str = "enableDnsHostnames";

/// The `VpcAttributeName` enum's `enableDnsSupport` member (see
/// [`VPC_ATTRIBUTE_ENABLE_DNS_HOSTNAMES`] for the generated catalog these come from).
pub const VPC_ATTRIBUTE_ENABLE_DNS_SUPPORT: &str = "enableDnsSupport";

/// The `VpcAttributeName` enum's `enableNetworkAddressUsageMetrics` member (see
/// [`VPC_ATTRIBUTE_ENABLE_DNS_HOSTNAMES`] for the generated catalog these come from).
pub const VPC_ATTRIBUTE_ENABLE_NETWORK_ADDRESS_USAGE_METRICS: &str =
    "enableNetworkAddressUsageMetrics";

/// The attributes of one VPC: one field per `VpcAttributeName` member, which is exactly
/// the set `DescribeVpcAttribute` reports and `ModifyVpcAttribute` sets.
///
/// Held in [`ComAmazonawsEc2Amazonec2State::vpc_attributes`] rather than on the modelled
/// [`Vpc`] shape, which carries no member for any of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VpcAttributes {
    /// `enableDnsSupport`: "whether the DNS resolution is supported for the VPC"
    /// (`ModifyVpcAttributeRequest$EnableDnsSupport`).
    pub enable_dns_support: bool,
    /// `enableDnsHostnames`: "whether the instances launched in the VPC get DNS
    /// hostnames" (`ModifyVpcAttributeRequest$EnableDnsHostnames`).
    pub enable_dns_hostnames: bool,
    /// `enableNetworkAddressUsageMetrics`: "whether Network Address Usage metrics are
    /// enabled for your VPC"
    /// (`ModifyVpcAttributeRequest$EnableNetworkAddressUsageMetrics`).
    pub enable_network_address_usage_metrics: bool,
}

impl VpcAttributes {
    /// The attributes a VPC is created with.
    ///
    /// Amazon VPC User Guide, "DNS attributes in your VPC": `enableDnsSupport` is `true`
    /// for a default VPC and for a VPC you create, while `enableDnsHostnames` is `true`
    /// for a default VPC and `false` for a VPC you create. Network Address Usage metrics
    /// are a VPC feature a caller turns on, so they start off.
    pub fn for_vpc(is_default: bool) -> Self {
        Self {
            enable_dns_support: true,
            enable_dns_hostnames: is_default,
            enable_network_address_usage_metrics: false,
        }
    }

    /// The value of the attribute `attribute` names, or `None` when that is not a
    /// `VpcAttributeName` member.
    pub fn value_of(&self, attribute: &str) -> Option<bool> {
        match attribute {
            VPC_ATTRIBUTE_ENABLE_DNS_HOSTNAMES => Some(self.enable_dns_hostnames),
            VPC_ATTRIBUTE_ENABLE_DNS_SUPPORT => Some(self.enable_dns_support),
            VPC_ATTRIBUTE_ENABLE_NETWORK_ADDRESS_USAGE_METRICS => {
                Some(self.enable_network_address_usage_metrics)
            }
            _ => None,
        }
    }
}

/// `Ipv6AddressAttribute` of a block advertised on the internet from AWS.
pub const IPV6_ADDRESS_PUBLIC: &str = "public";

/// `IpSource` of an IPv6 block Amazon allocated.
pub const IP_SOURCE_AMAZON: &str = "amazon";

/// `IpSource` of an IPv6 block the customer brought to AWS.
pub const IP_SOURCE_BYOIP: &str = "byoip";

/// The largest number of VPCs this in-memory store holds.
///
/// A capacity bound, not an AWS quota (AWS's VPCs-per-Region quota is adjustable): the
/// store lives in this process's memory, so the number of VPCs a caller can create has to
/// be finite. `CreateVpc` answers [`VPC_LIMIT_EXCEEDED`] rather than growing without
/// limit.
pub const MAX_VPCS: usize = 1_000;

/// EC2's error code for a `CreateVpc` that would take the account past its VPC quota.
pub const VPC_LIMIT_EXCEEDED: &str = "VpcLimitExceeded";

/// The largest number of subnets this in-memory store holds.
///
/// A capacity bound in the same style as [`MAX_VPCS`] and [`MAX_STORED_INSTANCES`], not an
/// AWS quota: each subnet is a record in this process's memory, and a default VPC alone
/// seeds one per Availability Zone, so the total has to be finite.
/// [`ComAmazonawsEc2Amazonec2State::has_subnet_capacity`] is the predicate a writer asks
/// before inserting; a create past it refuses with [`SUBNET_LIMIT_EXCEEDED`].
pub const MAX_SUBNETS: usize = 4_096;

/// EC2's error code for a create that would take the account past its subnet quota —
/// "You've reached the limit on the number of subnets that you can create" (EC2 API error
/// codes).
pub const SUBNET_LIMIT_EXCEEDED: &str = "SubnetLimitExceeded";

// ---------------------------------------------------------------------------
// CIDR blocks
// ---------------------------------------------------------------------------

/// The smallest IPv4 prefix length a VPC CIDR block may have.
///
/// Amazon VPC User Guide, VPC CIDR blocks: "The allowed block size is between a /16
/// netmask (65,536 IP addresses) and /28 netmask (16 IP addresses)."
pub const MIN_VPC_IPV4_PREFIX: u8 = 16;

/// The largest IPv4 prefix length a VPC CIDR block may have (see
/// [`MIN_VPC_IPV4_PREFIX`]).
pub const MAX_VPC_IPV4_PREFIX: u8 = 28;

/// Why a requested CIDR block cannot be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CidrProblem {
    /// The value is not a CIDR block of the expected address family at all.
    Malformed,
    /// The value parses, but its prefix length is outside the allowed block size.
    PrefixLength,
}

/// A requested IPv4 VPC CIDR block in the canonical form AWS stores it in.
///
/// `CreateVpcRequest$CidrBlock`: "We modify the specified CIDR block to its canonical
/// form; for example, if you specify 100.68.0.18/18, we modify it to 100.68.0.0/18" — so
/// the host bits are cleared rather than the value being refused. The prefix length is
/// held to the published block size ([`MIN_VPC_IPV4_PREFIX`]..=[`MAX_VPC_IPV4_PREFIX`]).
///
/// The block itself is read by [`canonical_ipv4_cidr`], which the subnet path
/// ([`canonical_subnet_ipv4_cidr`]) reads its own blocks with too, so this crate keeps ONE
/// rule for what an IPv4 CIDR block is and only the published block size differs between
/// the two. That reader in turn parses the address with the standard library's `Ipv4Addr`,
/// the same parser [`parse_ipv4_cidr_block`] uses; the only difference between those two
/// is that this path CLEARS the host bits where that one requires them already clear.
pub fn canonical_vpc_ipv4_cidr(value: &str) -> Result<String, CidrProblem> {
    let canonical = canonical_ipv4_cidr(value)?;
    let (_, prefix_length) = split_ipv4_cidr(&canonical).ok_or(CidrProblem::Malformed)?;
    if !(MIN_VPC_IPV4_PREFIX..=MAX_VPC_IPV4_PREFIX).contains(&prefix_length) {
        return Err(CidrProblem::PrefixLength);
    }
    Ok(canonical)
}

/// A requested IPv6 CIDR block in canonical form: the host bits cleared, the address
/// rendered the one way `Ipv6Addr` renders it.
///
/// Deliberately NO prefix-length rule, unlike the IPv4 path: the model states a size only
/// for the block AWS itself picks ("an Amazon-provided IPv6 CIDR block with a /56 prefix
/// length"), which this service mints at that size, while the size a caller may bring from
/// its own pool is a property of that pool and is documented nowhere the model or the
/// request carries. A bound invented here would refuse blocks AWS accepts.
pub fn canonical_ipv6_cidr(value: &str) -> Result<String, CidrProblem> {
    let (address, prefix_length) = value.split_once('/').ok_or(CidrProblem::Malformed)?;
    let base = u128::from(
        address
            .parse::<Ipv6Addr>()
            .map_err(|_| CidrProblem::Malformed)?,
    );
    let prefix_length: u8 = prefix_length.parse().map_err(|_| CidrProblem::Malformed)?;
    if prefix_length > 128 {
        return Err(CidrProblem::Malformed);
    }
    let mask = u128::MAX
        .checked_shl(u32::from(128 - prefix_length))
        .unwrap_or_default();
    Ok(format!("{}/{prefix_length}", Ipv6Addr::from(base & mask)))
}

// ---------------------------------------------------------------------------
// The rest of the EC2 error channel (the codes above live with the describes)
// ---------------------------------------------------------------------------

/// Refuse a request that leaves out a member the action requires, the way EC2 does: HTTP
/// 400 `MissingParameter`.
pub fn missing_parameter<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(MISSING_PARAMETER, 400, message)
}

/// Refuse members that are each valid but cannot be used together, the way EC2 does: HTTP
/// 400 `InvalidParameterCombination`.
pub fn invalid_parameter_combination<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(INVALID_PARAMETER_COMBINATION, 400, message)
}

/// Refuse a create that would take the store past [`MAX_VPCS`]: HTTP 400
/// `VpcLimitExceeded`.
pub fn vpc_limit_exceeded<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        VPC_LIMIT_EXCEEDED,
        400,
        format!("The maximum number of VPCs has been reached ({MAX_VPCS})"),
    )
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// [`service_state`], with the "mounted on an engine that holds no state" case
/// already turned into the server error every handler answers it with.
pub fn require_service_state<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
) -> Result<
    ::engine::ServiceState<ComAmazonawsEc2Amazonec2State>,
    crate::scaffolded::Scaffolded<E>,
> {
    service_state(engine).ok_or_else(|| {
        crate::scaffolded::Scaffolded::internal_error(
            "the EC2 emulator is mounted on an engine that holds no state",
        )
    })
}

// ---------------------------------------------------------------------------
// AMIs
// ---------------------------------------------------------------------------

/// An AMI that "is successfully registered and can be used to launch an instance"
/// (`Image$State`): the `ImageState` enum's `available` member.
pub const IMAGE_STATE_AVAILABLE: &str = "available";

/// An AMI its owner has disabled: the `ImageState` enum's `disabled` member. Such an
/// image is left out of a report unless the request asks for it (`IncludeDisabled`).
pub const IMAGE_STATE_DISABLED: &str = "disabled";

/// "The type of image" (`Image$ImageType`) for an AMI, as opposed to a kernel or a RAM
/// disk image: the `ImageTypeValues` enum's `machine` member.
pub const IMAGE_TYPE_MACHINE: &str = "machine";

/// "The hypervisor type of the image. Only `xen` is supported" (`Image$Hypervisor`).
pub const IMAGE_HYPERVISOR_XEN: &str = "xen";

/// The virtualization type of every image published today: the `VirtualizationType`
/// enum's `hvm` member (`paravirtual` is the retired alternative).
pub const IMAGE_VIRTUALIZATION_TYPE_HVM: &str = "hvm";

/// An AMI whose root volume is an Amazon EBS volume: the `DeviceType` enum's `ebs`
/// member (`Image$RootDeviceType`).
pub const IMAGE_ROOT_DEVICE_TYPE_EBS: &str = "ebs";

/// "A value of `simple` indicates that enhanced networking with the Intel 82599 VF
/// interface is enabled" (the `sriov-net-support` filter of `DescribeImages`).
pub const IMAGE_SRIOV_NET_SUPPORT_SIMPLE: &str = "simple";

/// The EBS volume type an image's root snapshot is restored into unless its spec names
/// another: the `VolumeType` enum's `gp3` member.
pub const IMAGE_ROOT_VOLUME_TYPE: &str = "gp3";

/// The EBS volume type of the older published images whose own names carry it (`…-gp2`):
/// the `VolumeType` enum's `gp2` member.
pub const IMAGE_ROOT_VOLUME_TYPE_GP2: &str = "gp2";

/// "Indicates whether deregistration protection is enabled for the AMI"
/// (`Image$DeregistrationProtection`) — off, nothing having enabled it.
pub const IMAGE_DEREGISTRATION_PROTECTION_DISABLED: &str = "disabled";

/// `Image$BootMode`'s `uefi-preferred`: the image boots UEFI where the instance type
/// supports it and legacy BIOS otherwise.
pub const BOOT_MODE_UEFI_PREFERRED: &str = "uefi-preferred";

/// `Image$BootMode`'s `uefi`.
pub const BOOT_MODE_UEFI: &str = "uefi";

/// `Image$BootMode`'s `legacy-bios`.
pub const BOOT_MODE_LEGACY_BIOS: &str = "legacy-bios";

/// "If `v2.0`, it indicates that IMDSv2 is specified in the AMI"
/// (`Image$ImdsSupport`).
pub const IMDS_SUPPORT_V2: &str = "v2.0";

/// "This value is set to `windows` for Windows AMIs; otherwise, it is blank"
/// (`Image$Platform`), whose modelled `PlatformValues` enum publishes the single value
/// `Windows` — so that is the value a stored image carries.
pub const IMAGE_PLATFORM_WINDOWS: &str = "Windows";

/// The `PlatformDetails` of a Linux image: "the platform details associated with the
/// billing code of the AMI" (`Image$PlatformDetails`).
pub const PLATFORM_DETAILS_LINUX: &str = "Linux/UNIX";

/// The `PlatformDetails` of a Windows image.
pub const PLATFORM_DETAILS_WINDOWS: &str = "Windows";

/// The `UsageOperation` billing code of a Linux image (`Image$UsageOperation`).
pub const USAGE_OPERATION_LINUX: &str = "RunInstances";

/// The `UsageOperation` billing code of a Windows image — Amazon's billing-code table
/// pairs `RunInstances:0002` with Windows ("Understand AMI billing information").
pub const USAGE_OPERATION_WINDOWS: &str = "RunInstances:0002";

/// The root device name of the published Linux images (`Image$RootDeviceName`).
pub const LINUX_ROOT_DEVICE_NAME: &str = "/dev/xvda";

/// The root device name of the published Windows images.
pub const WINDOWS_ROOT_DEVICE_NAME: &str = "/dev/sda1";

/// The root volume size, in GiB, of a published Linux image.
pub const LINUX_ROOT_VOLUME_SIZE: i32 = 8;

/// The root volume size, in GiB, of a published Windows image.
pub const WINDOWS_ROOT_VOLUME_SIZE: i32 = 30;

/// The owner alias Amazon's own published AMIs carry: "the owner alias (`amazon` |
/// `aws-backup-vault` | `aws-marketplace`)" (`Image$ImageOwnerAlias`).
pub const IMAGE_OWNER_ALIAS_AMAZON: &str = "amazon";

/// Every owner alias `DescribeImagesRequest$Owners` accepts beside an account id and
/// `self`: "you can specify a combination of Amazon Web Services account IDs, self,
/// amazon, aws-backup-vault, and aws-marketplace".
pub const IMAGE_OWNER_ALIASES: [&str; 3] = ["amazon", "aws-backup-vault", "aws-marketplace"];

/// The account id Amazon publishes its Amazon Linux AMIs from, which the published
/// catalog's images are owned by; `amazon` is the alias for it
/// (`IMAGE_OWNER_ALIAS_AMAZON`).
pub const AMAZON_IMAGE_OWNER_ID: &str = "137112412989";

/// "self (the sender of the request)" — the `Owners` and `ExecutableUsers` term that
/// names the calling account.
pub const OWNER_SELF: &str = "self";

/// "all (public AMIs)" — the `ExecutableUsers` term that names every account.
pub const EXECUTABLE_BY_ALL: &str = "all";

/// How many digits an Amazon Web Services account id has.
pub const AWS_ACCOUNT_ID_DIGITS: usize = 12;

/// How many characters of an ISO 8601 UTC timestamp carry the instant down to the
/// second: `YYYY-MM-DDThh:mm:ss`.
const ISO8601_SECONDS_WIDTH: usize = 19;

/// How many images this store holds, so a writer cannot grow it without bound. A create
/// asks [`ComAmazonawsEc2Amazonec2State::has_image_capacity`] first, the way a VPC create
/// asks [`ComAmazonawsEc2Amazonec2State::has_vpc_capacity`].
pub const MAX_IMAGES: usize = 4_096;

/// The code EC2 answers for an AMI id the account can see no image for.
pub const INVALID_AMI_ID_NOT_FOUND: &str = "InvalidAMIID.NotFound";

/// The code EC2 answers for something that is not an AMI id at all.
pub const INVALID_AMI_ID_MALFORMED: &str = "InvalidAMIID.Malformed";

/// The code EC2 answers for an owner or executable-by term that is neither an account id
/// nor one of the aliases — "you must specify a valid value for the owner or executableBy
/// parameters, such as an Amazon Web Services account ID" (EC2 error codes,
/// `InvalidUserID.Malformed`).
pub const INVALID_USER_ID_MALFORMED: &str = "InvalidUserID.Malformed";

/// Is `id` shaped like an AMI id?
///
/// An image id is `ami-` followed by 8 or 17 lowercase hexadecimal digits — the short and
/// the long id form, the same two forms [`instance_id_is_well_formed`] accepts. EC2
/// answers `InvalidAMIID.Malformed` for anything else, BEFORE it looks the id up, which is
/// a different answer from `InvalidAMIID.NotFound` for a well-formed id no image has.
pub fn image_id_is_well_formed(id: &str) -> bool {
    prefixed_id_is_well_formed("ami", id)
}

/// Is `id` shaped like an Amazon Web Services account id — [`AWS_ACCOUNT_ID_DIGITS`]
/// decimal digits?
pub fn aws_account_id_is_well_formed(id: &str) -> bool {
    id.len() == AWS_ACCOUNT_ID_DIGITS && id.bytes().all(|byte| byte.is_ascii_digit())
}

/// Does the ISO 8601 UTC timestamp `timestamp` name an instant before `reference`?
///
/// Both are `YYYY-MM-DDThh:mm:ss` with an optional fraction and the `Z` zone — a
/// fixed-width UTC rendering whose lexicographic order IS its chronological order — so the
/// comparison is made over the first [`ISO8601_SECONDS_WIDTH`] characters, which every
/// such rendering carries whether or not it also carries a fraction.
fn iso8601_is_before(timestamp: &str, reference: &str) -> bool {
    fn to_the_second(value: &str) -> &str {
        value
            .char_indices()
            .nth(ISO8601_SECONDS_WIDTH)
            .map_or(value, |(index, _)| &value[..index])
    }
    to_the_second(timestamp) < to_the_second(reference)
}

/// Is `image` deprecated as of `now` — does it carry a `DeprecationTime` that has passed?
///
/// "The date and time to deprecate the AMI" (`Image$DeprecationTime`): an AMI with no such
/// time is never deprecated, and one whose time is still in the future is not deprecated
/// yet, so both are reported without `IncludeDeprecated`.
pub fn image_is_deprecated(image: &Image, now: SystemTime) -> bool {
    let reference = iso8601_utc(now);
    image
        .deprecation_time
        .as_deref()
        .is_some_and(|at| iso8601_is_before(at, &reference))
}

/// Refuse a request that named an AMI id the account can see no image for, the way EC2
/// does: HTTP 400 `InvalidAMIID.NotFound`.
pub fn invalid_ami_id_not_found<E>(image_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_AMI_ID_NOT_FOUND,
        400,
        format!("The image id '{image_id}' does not exist"),
    )
}

/// Refuse a request that named something that is not an AMI id at all: HTTP 400
/// `InvalidAMIID.Malformed`.
pub fn invalid_ami_id_malformed<E>(image_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_AMI_ID_MALFORMED,
        400,
        format!("Invalid id: '{image_id}'"),
    )
}

/// Refuse an owner or executable-by term that names no account and no alias: HTTP 400
/// `InvalidUserID.Malformed`.
pub fn invalid_user_id_malformed<E>(user_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_USER_ID_MALFORMED,
        400,
        format!("Invalid user id: '{user_id}'"),
    )
}

/// What one AMI is stored with: the facts about the image itself, everything the service
/// derives being left to [`ComAmazonawsEc2Amazonec2State::insert_image`].
#[derive(Debug, Clone)]
pub struct ImageSpec {
    /// "The name of the AMI that was provided during image creation" (`Image$Name`).
    pub name: String,
    /// "The description of the AMI that was provided during image creation".
    pub description: String,
    /// One of `ARCHITECTURE_VALUES_VALUES`.
    pub architecture: String,
    /// `Some(`[`IMAGE_PLATFORM_WINDOWS`]`)` for a Windows image, `None` otherwise.
    pub platform: Option<String>,
    /// The billing platform details ([`PLATFORM_DETAILS_LINUX`]).
    pub platform_details: String,
    /// The billing usage operation ([`USAGE_OPERATION_LINUX`]).
    pub usage_operation: String,
    /// One of `BOOT_MODE_VALUES_VALUES`, for an image that declares one.
    pub boot_mode: Option<String>,
    /// [`IMDS_SUPPORT_V2`] for an image that requires IMDSv2.
    pub imds_support: Option<String>,
    /// The root device name of the image's block device mapping.
    pub root_device_name: String,
    /// The size, in GiB, of the root volume the image's snapshot restores into.
    pub root_volume_size: i32,
    /// The EBS volume type that volume is created as.
    pub root_volume_type: String,
    /// One of `IMAGE_STATE_VALUES`.
    pub state: String,
    /// "Indicates whether the image has public launch permissions" (`Image$Public`).
    pub public: bool,
    /// The account that owns the image.
    pub owner_id: String,
    /// The owner's alias, for an image published under one.
    pub owner_alias: Option<String>,
    /// "The date and time the image was created" (`Image$CreationDate`), an ISO 8601 UTC
    /// timestamp.
    pub creation_date: String,
    /// "The date and time to deprecate the AMI" (`Image$DeprecationTime`), for an image
    /// that carries one.
    pub deprecation_time: Option<String>,
    /// "Indicates whether the image is eligible for Amazon Web Services Free Tier".
    pub free_tier_eligible: bool,
    /// "The name of the public Systems Manager parameter that resolves to this AMI",
    /// for an image one resolves to.
    pub public_ssm_parameter_name: Option<String>,
    /// "Any tags assigned to the image" (`Image$Tags`).
    pub tags: Vec<Tag>,
}

/// An EBS-backed Linux machine image of the calling account, created now and available:
/// the shape of an image a register or create operation stores. Every published catalog
/// row below starts from [`published_amazon_image`], which starts from this.
impl Default for ImageSpec {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            architecture: "x86_64".to_string(),
            platform: None,
            platform_details: PLATFORM_DETAILS_LINUX.to_string(),
            usage_operation: USAGE_OPERATION_LINUX.to_string(),
            boot_mode: None,
            imds_support: None,
            root_device_name: LINUX_ROOT_DEVICE_NAME.to_string(),
            root_volume_size: LINUX_ROOT_VOLUME_SIZE,
            root_volume_type: IMAGE_ROOT_VOLUME_TYPE.to_string(),
            state: IMAGE_STATE_AVAILABLE.to_string(),
            public: false,
            owner_id: ::engine::emulator_account_id().to_string(),
            owner_alias: None,
            creation_date: iso8601_utc(SystemTime::now()),
            deprecation_time: None,
            free_tier_eligible: false,
            public_ssm_parameter_name: None,
            tags: Vec::new(),
        }
    }
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another image fits inside the store's ceiling ([`MAX_IMAGES`]).
    ///
    /// The same contract as [`Self::has_vpc_capacity`]: a create asks first and refuses
    /// instead of growing the store past its ceiling.
    pub fn has_image_capacity(&self) -> bool {
        self.images.len() < MAX_IMAGES
    }

    /// Store an AMI, and answer the `ImageId` it was given.
    ///
    /// Everything the service itself owns is derived here rather than taken from the
    /// caller — the image's own id, the snapshot id of its root block device mapping, the
    /// manifest location `<owner>/<name>` the `manifest-location` filter selects on, and
    /// the fixed facts of an EBS-backed HVM machine image — so two writers cannot report
    /// the same image differently. Like [`Self::insert_vpc`], the caller-chosen bytes of
    /// `spec.tags` are charged against the store's byte budget here.
    pub fn insert_image(&mut self, spec: ImageSpec) -> String {
        let image_id = self.next_id("ami");
        let snapshot_id = self.next_id("snap");
        let owner = spec
            .owner_alias
            .clone()
            .unwrap_or_else(|| spec.owner_id.clone());
        let image = Image {
            platform_details: Some(spec.platform_details),
            usage_operation: Some(spec.usage_operation),
            // "Any block device mapping entries" (`Image$BlockDeviceMappings`): the root
            // volume's, restored from the snapshot minted above.
            block_device_mappings: Some(vec![BlockDeviceMapping {
                ebs: Some(EbsBlockDevice {
                    delete_on_termination: Some(true),
                    snapshot_id: Some(snapshot_id),
                    volume_size: Some(spec.root_volume_size),
                    volume_type: Some(spec.root_volume_type),
                    encrypted: Some(false),
                    iops: None,
                    kms_key_id: None,
                    throughput: None,
                    outpost_arn: None,
                    availability_zone: None,
                    volume_initialization_rate: None,
                    availability_zone_id: None,
                    ebs_card_index: None,
                }),
                no_device: None,
                device_name: Some(spec.root_device_name.clone()),
                virtual_name: None,
            }]),
            description: Some(spec.description),
            ena_support: Some(true),
            hypervisor: Some(IMAGE_HYPERVISOR_XEN.to_string()),
            image_owner_alias: spec.owner_alias,
            name: Some(spec.name.clone()),
            root_device_name: Some(spec.root_device_name),
            root_device_type: Some(IMAGE_ROOT_DEVICE_TYPE_EBS.to_string()),
            sriov_net_support: Some(IMAGE_SRIOV_NET_SUPPORT_SIMPLE.to_string()),
            // "The reason for the state change" (`Image$StateReason`): an image that
            // never left `available` has had no state change to report.
            state_reason: None,
            tags: Some(spec.tags),
            virtualization_type: Some(IMAGE_VIRTUALIZATION_TYPE_HVM.to_string()),
            boot_mode: spec.boot_mode,
            // NitroTPM is not configured for any image this store holds.
            tpm_support: None,
            deprecation_time: spec.deprecation_time,
            imds_support: spec.imds_support,
            // Set only "if the AMI was created using CreateImage" (`Image$SourceInstanceId`).
            source_instance_id: None,
            deregistration_protection: Some(
                IMAGE_DEREGISTRATION_PROTECTION_DISABLED.to_string(),
            ),
            // "when the AMI was last used to launch an EC2 instance … there is a 24-hour
            // delay before that usage is reported" (`Image$LastLaunchedTime`), so an
            // image this store has only just published reports none.
            last_launched_time: None,
            // "If `true`, the AMI satisfies the criteria for Allowed AMIs"
            // (`Image$ImageAllowed`), which is the value of an account whose Allowed AMIs
            // criteria every image satisfies — among them an account that declares none,
            // which is what [`Self::allowed_images_settings`] starts empty at. Evaluating
            // stored criteria against an image is `DescribeImages`'s own behaviour and the
            // `ImageCriterion` matcher it would need does not exist yet, so nothing writes
            // another value here.
            image_allowed: Some(true),
            // Set only for an image copied from another (`CopyImage`).
            source_image_id: None,
            source_image_region: None,
            free_tier_eligible: Some(spec.free_tier_eligible),
            public_ssm_parameter_name: spec.public_ssm_parameter_name,
            // No watermark is attached until something attaches one.
            image_watermarks: Some(Vec::new()),
            // An image whose owner published no instance-type specification is
            // compatible with every instance type its architecture offers.
            instance_type_specification: None,
            image_id: Some(image_id.clone()),
            // "The location of the AMI" (`Image$ImageLocation`): `<owner>/<name>`, the
            // manifest location the `manifest-location` filter selects on.
            image_location: Some(format!("{owner}/{}", spec.name)),
            state: Some(spec.state),
            owner_id: Some(spec.owner_id),
            creation_date: Some(spec.creation_date),
            public: Some(spec.public),
            // No product code: nothing here is a Marketplace or paid image.
            product_codes: Some(Vec::new()),
            architecture: Some(spec.architecture),
            image_type: Some(IMAGE_TYPE_MACHINE.to_string()),
            // An HVM image carries neither a kernel nor a RAM disk image.
            kernel_id: None,
            ramdisk_id: None,
            platform: spec.platform,
        };
        self.add_stored_bytes(tag_list_bytes(image.tags.as_deref().unwrap_or_default()));
        self.images.insert(image_id.clone(), image);
        image_id
    }

    /// Give the store the public images Amazon publishes ([`published_image_specs`]), and
    /// answer their ids.
    ///
    /// "The images available to you include public images" (`DescribeImages`) and no EC2
    /// operation creates them, so they are part of a fresh account's state the same way
    /// its default VPC is.
    pub fn insert_published_images(&mut self) -> Vec<String> {
        let mut inserted = Vec::new();
        for spec in published_image_specs() {
            if !self.has_image_capacity() {
                break;
            }
            inserted.push(self.insert_image(spec));
        }
        inserted
    }
}

/// A public AMI Amazon publishes: owned by Amazon's publishing account
/// ([`AMAZON_IMAGE_OWNER_ID`]) under the `amazon` alias, with public launch permissions,
/// and eligible for the Free Tier as the Amazon Linux and Windows base images are.
fn published_amazon_image(name: &str, architecture: &str, creation_date: &str) -> ImageSpec {
    ImageSpec {
        name: name.to_string(),
        architecture: architecture.to_string(),
        creation_date: creation_date.to_string(),
        public: true,
        owner_id: AMAZON_IMAGE_OWNER_ID.to_string(),
        owner_alias: Some(IMAGE_OWNER_ALIAS_AMAZON.to_string()),
        free_tier_eligible: true,
        ..ImageSpec::default()
    }
}

/// The public AMIs a fresh account can already see.
///
/// Each row is one Amazon-published image, named the way Amazon documents its published
/// names: an Amazon Linux 2023 image is
/// `al2023-ami[-minimal]-<version>-kernel-<kernel version>-<architecture>` and an Amazon
/// Linux 2 image `amzn2-ami[-kernel-<kernel version>][-minimal]-hvm-<version>-<architecture>-<root volume>`
/// (Amazon Linux 2023 User Guide, "Find an AMI"), with the build date of that version as
/// its `CreationDate` — which is what makes "the newest `CreationDate` of the images
/// matching a name pattern" pick the same image on every run. Two builds of the same
/// variant are published, because that is the choice a caller resolving an AMI by name
/// pattern has to make, and the oldest Amazon Linux 2 build carries the
/// `DeprecationTime` its end of standard support fell on, so a deprecated image is part
/// of the catalog rather than only of a test.
pub fn published_image_specs() -> Vec<ImageSpec> {
    vec![
        ImageSpec {
            description: "Amazon Linux 2023 AMI 2023.6.20241121.0 x86_64 HVM kernel-6.1"
                .to_string(),
            boot_mode: Some(BOOT_MODE_UEFI_PREFERRED.to_string()),
            imds_support: Some(IMDS_SUPPORT_V2.to_string()),
            public_ssm_parameter_name: Some(
                "/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-6.1-x86_64".to_string(),
            ),
            ..published_amazon_image(
                "al2023-ami-2023.6.20241121.0-kernel-6.1-x86_64",
                "x86_64",
                "2024-11-21T04:40:03.000Z",
            )
        },
        ImageSpec {
            description: "Amazon Linux 2023 AMI 2023.6.20241121.0 arm64 HVM kernel-6.1"
                .to_string(),
            boot_mode: Some(BOOT_MODE_UEFI.to_string()),
            imds_support: Some(IMDS_SUPPORT_V2.to_string()),
            public_ssm_parameter_name: Some(
                "/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-6.1-arm64".to_string(),
            ),
            ..published_amazon_image(
                "al2023-ami-2023.6.20241121.0-kernel-6.1-arm64",
                "arm64",
                "2024-11-21T04:40:03.000Z",
            )
        },
        ImageSpec {
            description: "Amazon Linux 2023 AMI 2023.6.20241121.0 x86_64 HVM kernel-6.1 Minimal"
                .to_string(),
            boot_mode: Some(BOOT_MODE_UEFI_PREFERRED.to_string()),
            imds_support: Some(IMDS_SUPPORT_V2.to_string()),
            public_ssm_parameter_name: Some(
                "/aws/service/ami-amazon-linux-latest/al2023-ami-minimal-kernel-6.1-x86_64"
                    .to_string(),
            ),
            ..published_amazon_image(
                "al2023-ami-minimal-2023.6.20241121.0-kernel-6.1-x86_64",
                "x86_64",
                "2024-11-21T04:40:03.000Z",
            )
        },
        ImageSpec {
            description: "Amazon Linux 2023 AMI 2023.6.20241121.0 arm64 HVM kernel-6.1 Minimal"
                .to_string(),
            boot_mode: Some(BOOT_MODE_UEFI.to_string()),
            imds_support: Some(IMDS_SUPPORT_V2.to_string()),
            public_ssm_parameter_name: Some(
                "/aws/service/ami-amazon-linux-latest/al2023-ami-minimal-kernel-6.1-arm64"
                    .to_string(),
            ),
            ..published_amazon_image(
                "al2023-ami-minimal-2023.6.20241121.0-kernel-6.1-arm64",
                "arm64",
                "2024-11-21T04:40:03.000Z",
            )
        },
        // The previous build of the same two variants: an older `CreationDate`, and no
        // public Systems Manager parameter, which resolves to the latest build only.
        ImageSpec {
            description: "Amazon Linux 2023 AMI 2023.5.20240903.0 x86_64 HVM kernel-6.1"
                .to_string(),
            boot_mode: Some(BOOT_MODE_UEFI_PREFERRED.to_string()),
            imds_support: Some(IMDS_SUPPORT_V2.to_string()),
            ..published_amazon_image(
                "al2023-ami-2023.5.20240903.0-kernel-6.1-x86_64",
                "x86_64",
                "2024-09-03T22:17:11.000Z",
            )
        },
        ImageSpec {
            description: "Amazon Linux 2023 AMI 2023.5.20240903.0 x86_64 HVM kernel-6.1 Minimal"
                .to_string(),
            boot_mode: Some(BOOT_MODE_UEFI_PREFERRED.to_string()),
            imds_support: Some(IMDS_SUPPORT_V2.to_string()),
            ..published_amazon_image(
                "al2023-ami-minimal-2023.5.20240903.0-kernel-6.1-x86_64",
                "x86_64",
                "2024-09-03T22:17:11.000Z",
            )
        },
        ImageSpec {
            description: "Amazon Linux 2 Kernel 5.10 AMI 2.0.20241113.1 x86_64 HVM gp2"
                .to_string(),
            boot_mode: Some(BOOT_MODE_LEGACY_BIOS.to_string()),
            root_volume_type: IMAGE_ROOT_VOLUME_TYPE_GP2.to_string(),
            public_ssm_parameter_name: Some(
                "/aws/service/ami-amazon-linux-latest/amzn2-ami-kernel-5.10-hvm-x86_64-gp2"
                    .to_string(),
            ),
            ..published_amazon_image(
                "amzn2-ami-kernel-5.10-hvm-2.0.20241113.1-x86_64-gp2",
                "x86_64",
                "2024-11-13T18:21:48.000Z",
            )
        },
        ImageSpec {
            description: "Amazon Linux 2 AMI 2.0.20241113.1 x86_64 HVM ebs Minimal".to_string(),
            boot_mode: Some(BOOT_MODE_LEGACY_BIOS.to_string()),
            root_volume_type: IMAGE_ROOT_VOLUME_TYPE_GP2.to_string(),
            ..published_amazon_image(
                "amzn2-ami-minimal-hvm-2.0.20241113.1-x86_64-ebs",
                "x86_64",
                "2024-11-13T18:21:48.000Z",
            )
        },
        // Amazon Linux 2 reached the end of its standard support on 2025-06-30 (Amazon
        // Linux FAQs), which is the `DeprecationTime` the builds from before it carry.
        ImageSpec {
            description: "Amazon Linux 2 AMI 2.0.20230119.1 x86_64 HVM gp2".to_string(),
            boot_mode: Some(BOOT_MODE_LEGACY_BIOS.to_string()),
            root_volume_type: IMAGE_ROOT_VOLUME_TYPE_GP2.to_string(),
            deprecation_time: Some("2025-06-30T00:00:00.000Z".to_string()),
            ..published_amazon_image(
                "amzn2-ami-hvm-2.0.20230119.1-x86_64-gp2",
                "x86_64",
                "2023-01-19T18:33:27.000Z",
            )
        },
        ImageSpec {
            description: "Microsoft Windows Server 2022 Full Locale English AMI provided by Amazon"
                .to_string(),
            platform: Some(IMAGE_PLATFORM_WINDOWS.to_string()),
            platform_details: PLATFORM_DETAILS_WINDOWS.to_string(),
            usage_operation: USAGE_OPERATION_WINDOWS.to_string(),
            boot_mode: Some(BOOT_MODE_UEFI_PREFERRED.to_string()),
            root_device_name: WINDOWS_ROOT_DEVICE_NAME.to_string(),
            root_volume_size: WINDOWS_ROOT_VOLUME_SIZE,
            public_ssm_parameter_name: Some(
                "/aws/service/ami-windows-latest/Windows_Server-2022-English-Full-Base"
                    .to_string(),
            ),
            ..published_amazon_image(
                "Windows_Server-2022-English-Full-Base-2024.11.13",
                "x86_64",
                "2024-11-13T07:36:19.000Z",
            )
        },
    ]
}

// ---------------------------------------------------------------------------
// Application status: what `DescribeApplicationStatus` aggregates
// ---------------------------------------------------------------------------

/// `ApplicationStatusEnum`'s `ok` member. The full catalog is the generated
/// `crate::ops::types::APPLICATION_STATUS_ENUM_VALUES`; these constants name the members
/// the aggregation below chooses between, so one value is spelled one way wherever it is
/// read or written.
pub const APPLICATION_STATUS_OK: &str = "ok";

/// `ApplicationStatusEnum`'s `impaired` member (see [`APPLICATION_STATUS_OK`]).
pub const APPLICATION_STATUS_IMPAIRED: &str = "impaired";

/// `ApplicationStatusEnum`'s `initializing` member (see [`APPLICATION_STATUS_OK`]).
pub const APPLICATION_STATUS_INITIALIZING: &str = "initializing";

/// `ApplicationStatusEnum`'s `insufficient-data` member (see [`APPLICATION_STATUS_OK`]).
pub const APPLICATION_STATUS_INSUFFICIENT_DATA: &str = "insufficient-data";

/// `ApplicationStatusEnum`'s `not-applicable` member (see [`APPLICATION_STATUS_OK`]).
pub const APPLICATION_STATUS_NOT_APPLICABLE: &str = "not-applicable";

/// `ApplicationStatusEnum`'s `suppressed` member (see [`APPLICATION_STATUS_OK`]).
pub const APPLICATION_STATUS_SUPPRESSED: &str = "suppressed";

/// `ApplicationStatusCheckEnum`'s `passed` member: one CHECK's own status, which is a
/// different enum from the instance-level [`APPLICATION_STATUS_OK`] above. The full
/// catalog is `crate::ops::types::APPLICATION_STATUS_CHECK_ENUM_VALUES`.
pub const APPLICATION_STATUS_CHECK_PASSED: &str = "passed";

/// `ApplicationStatusCheckEnum`'s `failed` member (see
/// [`APPLICATION_STATUS_CHECK_PASSED`]).
pub const APPLICATION_STATUS_CHECK_FAILED: &str = "failed";

/// `ApplicationStatusCheckEnum`'s `initializing` member (see
/// [`APPLICATION_STATUS_CHECK_PASSED`]).
pub const APPLICATION_STATUS_CHECK_INITIALIZING: &str = "initializing";

/// `ApplicationStatusCheckEnum`'s `insufficient-data` member (see
/// [`APPLICATION_STATUS_CHECK_PASSED`]).
pub const APPLICATION_STATUS_CHECK_INSUFFICIENT_DATA: &str = "insufficient-data";

/// `ApplicationStatusCheckEnum`'s `not-applicable` member (see
/// [`APPLICATION_STATUS_CHECK_PASSED`]): a check that does not apply to the instance, so
/// it is not one of the checks the instance-level status is derived from.
pub const APPLICATION_STATUS_CHECK_NOT_APPLICABLE: &str = "not-applicable";

/// `AggregationStatusEnum`'s `included` member: a check whose result "determines the
/// instance-level application status returned by DescribeApplicationStatus"
/// (`CreateApplicationStatusCheckRequest$Aggregation`). The full catalog is
/// `crate::ops::types::AGGREGATION_STATUS_ENUM_VALUES`.
pub const AGGREGATION_INCLUDED: &str = "included";

/// `AggregationStatusEnum`'s `excluded` member (see [`AGGREGATION_INCLUDED`]): "Checks
/// with `Aggregation` set to `excluded` do not affect this value"
/// (`ApplicationStatus$Status`).
pub const AGGREGATION_EXCLUDED: &str = "excluded";

/// A suppression of application status reporting for one instance.
///
/// `EnableApplicationStatusCheckSuppression` puts one in force and
/// `DisableApplicationStatusCheckSuppression` lifts it; while it is in force the
/// instance-level status is [`APPLICATION_STATUS_SUPPRESSED`] whatever the checks report.
#[derive(Debug, Clone)]
pub struct ApplicationStatusSuppression {
    /// When the suppression took effect: the `StatusSince` of the `suppressed` status it
    /// produces ("The date and time when the current status started",
    /// `ApplicationStatus$StatusSince`).
    pub since: SystemTime,
    /// When reporting resumes, reported as `ApplicationStatus$ResumeAt` ("The date and
    /// time when application status reporting resumes after suppression"). `None` when
    /// the request named no duration, which "suppresses health check results
    /// indefinitely until you call DisableApplicationStatusCheckSuppression"
    /// (`EnableApplicationStatusCheckSuppressionRequest$DurationSeconds`) — so there is
    /// no instant to report.
    pub resume_at: Option<SystemTime>,
}

impl ApplicationStatusSuppression {
    /// Whether this suppression is still in force at `now`.
    ///
    /// A suppression with an end stops suppressing once the clock has passed it, whether
    /// or not anybody called in the meantime, so a reader DERIVES that from the stored
    /// instant instead of writing the expiry back.
    pub fn in_force_at(&self, now: SystemTime) -> bool {
        self.resume_at.is_none_or(|resume_at| now < resume_at)
    }
}

/// What one instance's application health status is derived from.
///
/// The per-check results are stored as the MODELLED [`ApplicationStatusDetail`] the
/// describe reports, keyed by `ApplicationStatusCheckId`, so one check holds exactly one
/// result per instance and no projection can drop a member of it. The instance-level
/// status is NOT stored: it is a function of these details and of the suppression
/// ([`aggregated_application_status`]), so two operations cannot disagree about it.
#[derive(Debug, Clone, Default)]
pub struct InstanceApplicationStatusRecord {
    /// The result of every application status check associated with this instance, by
    /// `ApplicationStatusCheckId`.
    pub details: BTreeMap<String, ApplicationStatusDetail>,
    /// The suppression in force for this instance, if any.
    pub suppression: Option<ApplicationStatusSuppression>,
}

/// A modelled timestamp as a sortable key, so two of them can be ordered without
/// assuming anything about the timestamp type's own comparison traits.
fn date_time_key(at: ::aws_smithy_types::DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos())
}

/// When one check's current result started: its `StatusSince` ("The date and time when
/// the current status started"), falling back to its `StatusTimeStamp` for a result that
/// carries no `StatusSince`.
fn detail_since(detail: &ApplicationStatusDetail) -> Option<::aws_smithy_types::DateTime> {
    detail.status_since.or(detail.status_time_stamp)
}

/// "The date and time of the last status update" (`ApplicationStatus$StatusTimeStamp`)
/// for an instance whose checks reported `details`: the most recent update any of them
/// carries, included or excluded — every check of the instance is one of its status
/// updates, while only the included ones decide the instance-level status.
pub fn last_application_status_update(
    details: &[ApplicationStatusDetail],
) -> Option<::aws_smithy_types::DateTime> {
    details
        .iter()
        .filter_map(|detail| detail.status_time_stamp.or(detail.check_update_time))
        .max_by_key(|at| date_time_key(*at))
}

/// The instance-level application status, and the instant it started, derived from the
/// per-check `details` of ONE instance.
///
/// `ApplicationStatus$Status`'s own documentation is the whole rule: `impaired` when "at
/// least one included check failed", `initializing` when "at least one included check is
/// initializing, and no included check is impaired", `insufficient-data` when "at least
/// one included check has insufficient data, and no included check is impaired or
/// initializing", `ok` when "all included checks passed", and `not-applicable` when "no
/// checks with `Aggregation` set to `included` apply to the instance" — a check reporting
/// `not-applicable` being one that does not apply, and "checks with `Aggregation` set to
/// `excluded` do not affect this value".
///
/// The instant is when that predicate began to hold, read off the per-check
/// `StatusSince`: the EARLIEST of the checks that satisfy an "at least one" predicate,
/// and for `ok` the LATEST of them, which is when the last check turned `passed`. For
/// `not-applicable` no check carries one, so the answer is `None` and the caller supplies
/// the instant the instance has had no applicable check since.
pub fn aggregated_application_status(
    details: &[ApplicationStatusDetail],
) -> (&'static str, Option<::aws_smithy_types::DateTime>) {
    let with_status = |status: &'static str| -> Vec<&ApplicationStatusDetail> {
        details
            .iter()
            .filter(|detail| detail.aggregation.as_deref() == Some(AGGREGATION_INCLUDED))
            .filter(|detail| detail.status.as_deref() == Some(status))
            .collect()
    };

    // The precedence the member documents, worst first: one failed check impairs the
    // instance however many others pass, and an initializing one outranks a check with
    // insufficient data.
    for (check_status, instance_status) in [
        (APPLICATION_STATUS_CHECK_FAILED, APPLICATION_STATUS_IMPAIRED),
        (
            APPLICATION_STATUS_CHECK_INITIALIZING,
            APPLICATION_STATUS_INITIALIZING,
        ),
        (
            APPLICATION_STATUS_CHECK_INSUFFICIENT_DATA,
            APPLICATION_STATUS_INSUFFICIENT_DATA,
        ),
    ] {
        let matching = with_status(check_status);
        if !matching.is_empty() {
            let started = matching
                .iter()
                .filter_map(|detail| detail_since(detail))
                .min_by_key(|at| date_time_key(*at));
            return (instance_status, started);
        }
    }

    // "ok – All included checks passed", which the instance has been only since the last
    // of them turned `passed`.
    let passed = with_status(APPLICATION_STATUS_CHECK_PASSED);
    if !passed.is_empty() {
        let started = passed
            .iter()
            .filter_map(|detail| detail_since(detail))
            .max_by_key(|at| date_time_key(*at));
        return (APPLICATION_STATUS_OK, started);
    }

    // No included check applies: the instance has none, or every one it has reports
    // `not-applicable` (or a status this emulator does not read), and an excluded check
    // is not one that applies either.
    (APPLICATION_STATUS_NOT_APPLICABLE, None)
}

/// The `ZoneId` of the Availability Zone named `zone_name`.
///
/// A zone's name and its id are the SAME derived pair ([`availability_zones_in_region`]),
/// so the id is read back out of the Region the name belongs to rather than stored a
/// second time beside it. `None` for a name no Region of that form has a zone for.
pub fn zone_id_for_zone_name(zone_name: &str) -> Option<String> {
    let region = zone_name.strip_suffix(|letter: char| letter.is_ascii_lowercase())?;
    availability_zones_in_region(region)
        .into_iter()
        .find(|zone| zone.zone_name == zone_name)
        .map(|zone| zone.zone_id)
}

impl ComAmazonawsEc2Amazonec2State {
    /// The modelled `InstanceApplicationStatus` for one stored instance, as of `now`.
    ///
    /// Everything in it is derived from what the store holds — the instance's zone and
    /// tags, the per-check results written for it, and the suppression in force — so this
    /// is a READ: an instance nothing has reported a check for is reported
    /// `not-applicable`, which is the status the model documents for exactly that case,
    /// rather than given one here.
    pub fn instance_application_status(
        &self,
        instance: &InstanceRecord,
        now: SystemTime,
    ) -> InstanceApplicationStatus {
        let record = self.application_status.get(&instance.instance_id);
        let details: Vec<ApplicationStatusDetail> = record
            .map(|record| record.details.values().cloned().collect())
            .unwrap_or_default();
        // A suppression whose end the clock has passed suppresses nothing any more.
        let suppression = record
            .and_then(|record| record.suppression.as_ref())
            .filter(|suppression| suppression.in_force_at(now));

        let application_status = match suppression {
            // "suppressed – Application status reporting is suppressed for the
            // instance": the suppression, not the checks, is what the instance-level
            // status reports while it is in force, and `ResumeAt` is when reporting
            // resumes (absent while the suppression has no end).
            Some(suppression) => {
                let suppressed_since = system_time_to_date_time(suppression.since);
                ApplicationStatus {
                    status: Some(APPLICATION_STATUS_SUPPRESSED.to_string()),
                    // "The date and time of the last status update" is the latest of them
                    // whichever side it came from: a check that reported while the
                    // instance was suppressed is still a status update of it.
                    status_time_stamp: Some(
                        last_application_status_update(&details)
                            .filter(|updated| {
                                date_time_key(*updated) > date_time_key(suppressed_since)
                            })
                            .unwrap_or(suppressed_since),
                    ),
                    status_since: Some(suppressed_since),
                    resume_at: suppression.resume_at.map(system_time_to_date_time),
                    details: Some(details),
                }
            }
            None => {
                let (status, started) = aggregated_application_status(&details);
                // While no check has reported anything, the instance has had no
                // applicable check — and so this status — since it came into being.
                let launched_at = system_time_to_date_time(instance.launch_time);
                ApplicationStatus {
                    status: Some(status.to_string()),
                    status_time_stamp: Some(
                        last_application_status_update(&details).unwrap_or(launched_at),
                    ),
                    status_since: Some(started.unwrap_or(launched_at)),
                    // Reporting is not suppressed, so nothing is waiting to resume.
                    resume_at: None,
                    details: Some(details),
                }
            }
        };

        InstanceApplicationStatus {
            instance_id: Some(instance.instance_id.clone()),
            availability_zone: Some(instance.placement_az.clone()),
            availability_zone_id: zone_id_for_zone_name(&instance.placement_az),
            application_status: Some(application_status),
            tags: Some(tag_list(&instance.tags)),
        }
    }
}

// ---------------------------------------------------------------------------
// Pagination tokens
// ---------------------------------------------------------------------------

/// The marker every token this service mints carries, ahead of the checksum and the
/// cursor itself: a token a caller assembled carries neither, so it is detectable rather
/// than read as a cursor.
const PAGE_TOKEN_VERSION: &str = "ec2-1";

/// Encode a pagination cursor — the resource id the NEXT page starts at — as an opaque
/// token.
///
/// The token is the hex of `<version>:<checksum>:<id>`, so it is opaque to the caller and
/// a token this service did not mint fails verification in [`decode_page_token`] instead
/// of being treated as an id. Hex alone would not do that: any even-length hex string
/// decodes to something.
pub fn encode_page_token(start_at: &str) -> String {
    let checksum = page_token_checksum(start_at);
    ::hex::encode(format!("{PAGE_TOKEN_VERSION}:{checksum:016x}:{start_at}").as_bytes())
}

/// Decode a pagination cursor minted by [`encode_page_token`]; `None` for anything else —
/// a token that is not hex, does not carry this service's marker, or whose checksum does
/// not match the cursor it carries.
pub fn decode_page_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let marked = decoded
        .strip_prefix(PAGE_TOKEN_VERSION)
        .and_then(|rest| rest.strip_prefix(':'))?;
    let (checksum, start_at) = marked.split_once(':')?;
    let expected = format!("{:016x}", page_token_checksum(start_at));
    (checksum == expected).then(|| start_at.to_string())
}

/// The checksum a token carries over its cursor.
///
/// A token is only ever presented back to the process that minted it — pagination is
/// resumed within one run — so the standard library's hasher is enough to tell a token
/// this service built from one a caller assembled. It is not a secret and nothing depends
/// on it being unforgeable: the cursor it protects is a resource id of the caller's own
/// account, so the check exists to refuse a malformed token, not to withhold anything.
fn page_token_checksum(start_at: &str) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = ::std::hash::DefaultHasher::new();
    PAGE_TOKEN_VERSION.hash(&mut hasher);
    start_at.hash(&mut hasher);
    hasher.finish()
}

// ---------------------------------------------------------------------------
// The VPC-scoped network resource lifecycle
// ---------------------------------------------------------------------------
//
// A subnet and a security group are resources INSIDE a VPC: each is created into one
// named VPC, reported by that resource's describe, and deleted once nothing depends on
// it — and the VPC itself can only be deleted once it holds none of them, because
// "you must detach or delete all gateways and resources that are associated with the
// VPC before you can delete it … When you delete the VPC, it deletes the default
// security group, network ACL, and route table for the VPC" (`DeleteVpc`). What those
// operations share — the dependency question, the removal that gives the record's
// bytes back, and the error codes a refusal carries — lives here, so one rule cannot be
// enforced two ways. The security-group CREATE path's own helpers (its text alphabet,
// its id form) are below, beside the group create that reads them.

/// A subnet AWS is still creating: the `SubnetState` enum's `pending` member
/// (`smithy_query(query="shape",shape="SubnetState")`), which is what `CreateSubnet`
/// answers with until [`TRANSITION_DELAY`] has passed — the subnet counterpart of
/// [`VPC_STATE_PENDING`].
pub const SUBNET_STATE_PENDING: &str = "pending";

/// The `ResourceType` a `CreateSubnet` tag specification may name: the subnet it creates.
pub const RESOURCE_TYPE_SUBNET: &str = "subnet";

/// The `ResourceType` a `CreateSecurityGroup` tag specification may name: the group it
/// creates (`ResourceType` enum member `security-group`).
pub const RESOURCE_TYPE_SECURITY_GROUP: &str = "security-group";

/// How many characters a security group name or description may carry: "Up to 255
/// characters in length" (`CreateSecurityGroupRequest$GroupName` and `$Description`).
/// Both members carry the same bound, which is why one constant serves both.
pub const MAX_SECURITY_GROUP_TEXT_CHARS: usize = 255;

/// The punctuation a security group name or description may carry beside letters, digits
/// and spaces: "Valid characters: a-z, A-Z, 0-9, spaces, and ._-:/()#,@[]+=&;{}!$*"
/// (`CreateSecurityGroupRequest$GroupName` and `$Description`).
pub const SECURITY_GROUP_PUNCTUATION: &str = "._-:/()#,@[]+=&;{}!$*";

/// The prefix a security group name may NOT start with: "Can't start with sg-"
/// (`CreateSecurityGroupRequest$GroupName`), because that is the shape of a group id.
pub const SECURITY_GROUP_ID_PREFIX: &str = "sg-";

/// The largest number of security groups this in-memory store holds.
///
/// A capacity bound in the same style as [`MAX_VPCS`], not an AWS quota: each group is a
/// record in this process's memory, so the number a caller can create has to be finite.
/// [`ComAmazonawsEc2Amazonec2State::has_security_group_capacity`] is the predicate a
/// writer asks first; a create past it refuses with [`SECURITY_GROUP_LIMIT_EXCEEDED`].
pub const MAX_SECURITY_GROUPS: usize = 2_500;

/// EC2's error code for a create that would take the account past its security group
/// quota (EC2 API error codes, "SecurityGroupLimitExceeded").
pub const SECURITY_GROUP_LIMIT_EXCEEDED: &str = "SecurityGroupLimitExceeded";

/// EC2's error code for a delete the resource's own dependencies forbid — "the operation
/// fails with `DependencyViolation`" (`DeleteSecurityGroup`).
pub const DEPENDENCY_VIOLATION: &str = "DependencyViolation";

/// EC2's error code for a delete of a resource the service owns rather than the caller:
/// the default security group a VPC comes with, which only `DeleteVpc` removes.
pub const CANNOT_DELETE: &str = "CannotDelete";

/// EC2's error code for a subnet CIDR block that overlaps one already in the VPC: "A
/// subnet CIDR block must not overlap the CIDR block of an existing subnet in the VPC"
/// (`CreateSubnet`).
pub const INVALID_SUBNET_CONFLICT: &str = "InvalidSubnet.Conflict";

/// EC2's error code for a subnet CIDR block that is not inside its VPC's: a subnet is a
/// range OF the VPC's range, so a block outside it names no part of the VPC.
pub const INVALID_SUBNET_RANGE: &str = "InvalidSubnet.Range";

/// The smallest IPv4 prefix length a SUBNET CIDR block may have: "The allowed size for an
/// IPv4 subnet is between a /28 netmask (16 IP addresses) and a /16 netmask (65,536 IP
/// addresses)" (`CreateSubnet`) — the same published bounds as a VPC's own block, named
/// separately because they come from a different sentence and could move apart.
pub const MIN_SUBNET_IPV4_PREFIX: u8 = 16;

/// The largest IPv4 prefix length a subnet CIDR block may have (see
/// [`MIN_SUBNET_IPV4_PREFIX`]).
pub const MAX_SUBNET_IPV4_PREFIX: u8 = 28;

/// A requested IPv4 SUBNET CIDR block in the canonical form AWS stores it in.
///
/// "We modify the specified CIDR block to its canonical form; for example, if you specify
/// 100.68.0.18/18, we modify it to 100.68.0.0/18" (`CreateSubnetRequest$CidrBlock`) — the
/// same canonicalisation as a VPC's block, held to the subnet block size
/// ([`MIN_SUBNET_IPV4_PREFIX`]..=[`MAX_SUBNET_IPV4_PREFIX`]).
pub fn canonical_subnet_ipv4_cidr(value: &str) -> Result<String, CidrProblem> {
    let canonical = canonical_ipv4_cidr(value)?;
    let (_, prefix_length) = split_ipv4_cidr(&canonical).ok_or(CidrProblem::Malformed)?;
    if !(MIN_SUBNET_IPV4_PREFIX..=MAX_SUBNET_IPV4_PREFIX).contains(&prefix_length) {
        return Err(CidrProblem::PrefixLength);
    }
    Ok(canonical)
}

/// The smallest IPv4 prefix length a SECONDARY NETWORK CIDR block may have: "The allowed
/// size for a secondary network CIDR block is between /28 netmask (16 IP addresses) and /12
/// netmask (1,048,576 IP addresses)" (`CreateSecondaryNetwork`), also stated on the member
/// itself — "The CIDR block size must be between /12 and /28"
/// (`CreateSecondaryNetworkRequest$Ipv4CidrBlock`). Named separately from the VPC and
/// subnet bounds because it comes from a different sentence.
pub const MIN_SECONDARY_NETWORK_IPV4_PREFIX: u8 = 12;

/// The largest IPv4 prefix length a secondary network CIDR block may have (see
/// [`MIN_SECONDARY_NETWORK_IPV4_PREFIX`]).
pub const MAX_SECONDARY_NETWORK_IPV4_PREFIX: u8 = 28;

/// A requested IPv4 SECONDARY NETWORK CIDR block in the canonical form AWS stores it in:
/// the same canonicalisation as a VPC's and a subnet's block, held to the secondary
/// network block size
/// ([`MIN_SECONDARY_NETWORK_IPV4_PREFIX`]..=[`MAX_SECONDARY_NETWORK_IPV4_PREFIX`]).
pub fn canonical_secondary_network_ipv4_cidr(value: &str) -> Result<String, CidrProblem> {
    let canonical = canonical_ipv4_cidr(value)?;
    let (_, prefix_length) = split_ipv4_cidr(&canonical).ok_or(CidrProblem::Malformed)?;
    if !(MIN_SECONDARY_NETWORK_IPV4_PREFIX..=MAX_SECONDARY_NETWORK_IPV4_PREFIX)
        .contains(&prefix_length)
    {
        return Err(CidrProblem::PrefixLength);
    }
    Ok(canonical)
}

/// `value` as an IPv4 CIDR block with its host bits cleared, with NO rule on the prefix
/// length: the one reader [`canonical_vpc_ipv4_cidr`], [`canonical_subnet_ipv4_cidr`] and
/// [`canonical_secondary_network_ipv4_cidr`] put their own published block size on top of.
fn canonical_ipv4_cidr(value: &str) -> Result<String, CidrProblem> {
    let (address, prefix_length) = value.split_once('/').ok_or(CidrProblem::Malformed)?;
    let base = u32::from(
        address
            .parse::<::std::net::Ipv4Addr>()
            .map_err(|_| CidrProblem::Malformed)?,
    );
    let prefix_length: u8 = prefix_length.parse().map_err(|_| CidrProblem::Malformed)?;
    if prefix_length > 32 {
        // No IPv4 block has a prefix longer than its address, so the value is malformed
        // rather than a block of a disallowed size.
        return Err(CidrProblem::Malformed);
    }
    let mask = u32::MAX
        .checked_shl(u32::from(32 - prefix_length))
        .unwrap_or_default();
    Ok(format!(
        "{}/{prefix_length}",
        ::std::net::Ipv4Addr::from(base & mask)
    ))
}

/// `block` as its base address and prefix length, for a block already in canonical form.
fn split_ipv4_cidr(block: &str) -> Option<(u32, u8)> {
    let (address, prefix_length) = block.split_once('/')?;
    let base = u32::from(address.parse::<::std::net::Ipv4Addr>().ok()?);
    let prefix_length: u8 = prefix_length.parse().ok()?;
    (prefix_length <= 32).then_some((base, prefix_length))
}

/// The first and last address of `block`, or `None` when it is not an IPv4 CIDR block.
fn ipv4_cidr_bounds(block: &str) -> Option<(u32, u32)> {
    let (base, prefix_length) = split_ipv4_cidr(block)?;
    let size = 1u64 << (32 - u32::from(prefix_length));
    let last = u64::from(base).checked_add(size - 1)?;
    Some((base, u32::try_from(last).ok()?))
}

/// Do `left` and `right` share any address?
///
/// Two CIDR blocks of the same family overlap exactly when one's range meets the other's,
/// which is the question "A subnet CIDR block must not overlap the CIDR block of an
/// existing subnet in the VPC" (`CreateSubnet`) asks. A block this emulator cannot read
/// overlaps nothing, rather than being guessed at.
pub fn ipv4_cidr_blocks_overlap(left: &str, right: &str) -> bool {
    match (ipv4_cidr_bounds(left), ipv4_cidr_bounds(right)) {
        (Some((left_first, left_last)), Some((right_first, right_last))) => {
            left_first <= right_last && right_first <= left_last
        }
        _ => false,
    }
}

/// Is every address of `inner` an address of `outer`?
///
/// A subnet's block has to be a block OF its VPC's block, so this is the containment the
/// create path checks before the overlap one. An unreadable block contains nothing and is
/// contained by nothing.
pub fn ipv4_cidr_block_contains(outer: &str, inner: &str) -> bool {
    match (ipv4_cidr_bounds(outer), ipv4_cidr_bounds(inner)) {
        (Some((outer_first, outer_last)), Some((inner_first, inner_last))) => {
            outer_first <= inner_first && inner_last <= outer_last
        }
        _ => false,
    }
}

/// The tags a create assigns to the ONE resource it creates, from the request's
/// `TagSpecifications`.
///
/// "You must specify the resource type to tag, otherwise the request will fail"
/// (`TagSpecification`), and the only type an action may name is the resource it creates —
/// so a specification naming anything else is refused rather than silently dropped.
///
/// The tag payload is bounded here the way `CreateTags` bounds its own: at most the
/// documented number of tags per resource, each key and value within its documented
/// length, no key given twice ("Tag keys must be unique per resource", so two entries for
/// one key have no defined winner), and no key in the reserved `aws:` namespace. A count
/// of resources is not a bound on the caller-chosen bytes each one holds, which is why
/// every create charges [`tag_list_bytes`] of the result against the store's byte budget.
pub fn tags_from_specifications<E>(
    specifications: Option<&[TagSpecification]>,
    resource_type: &str,
    operation: &str,
) -> Result<Vec<Tag>, Scaffolded<E>> {
    let mut tags: Vec<Tag> = Vec::new();
    for specification in specifications.unwrap_or_default() {
        match specification.resource_type.as_deref() {
            Some(named) if named == resource_type => {
                tags.extend(specification.tags.clone().unwrap_or_default());
            }
            Some(other) => {
                return Err(invalid_parameter_value(format!(
                    "'{other}' is not a valid resource type for tagging on {operation}"
                )))
            }
            None => {
                return Err(missing_parameter(
                    "Tag specification resource type must have a value",
                ))
            }
        }
    }
    if tags.len() > MAX_TAGS_PER_RESOURCE {
        return Err(Scaffolded::tag_limit_exceeded(format!(
            "The request carries {} tags, above the maximum of {max} per resource",
            tags.len(),
            max = MAX_TAGS_PER_RESOURCE,
        )));
    }
    let mut keys_seen: BTreeSet<&str> = BTreeSet::new();
    for tag in &tags {
        let key = tag.key.as_deref().unwrap_or_default();
        if key.is_empty() {
            return Err(invalid_parameter_value("A tag key cannot be empty"));
        }
        if key.starts_with(RESERVED_TAG_PREFIX) {
            return Err(invalid_parameter_value(format!(
                "Tag key '{key}' is invalid: the '{prefix}' prefix is reserved",
                prefix = RESERVED_TAG_PREFIX,
            )));
        }
        if !keys_seen.insert(key) {
            return Err(invalid_parameter_value(format!(
                "Tag key '{key}' is given more than once; tag keys must be unique"
            )));
        }
        validate_tag_size(key, tag.value.as_deref().unwrap_or_default())?;
    }
    Ok(tags)
}

/// Refuse a delete the resource's own dependencies forbid: HTTP 400
/// `DependencyViolation`. `message` names the resource and what still depends on it, so
/// the caller knows what to delete first.
pub fn dependency_violation<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(DEPENDENCY_VIOLATION, 400, message)
}

/// Refuse a delete of the default security group of a VPC: HTTP 400 `CannotDelete`. That
/// group exists for as long as its VPC does — "When you delete the VPC, it deletes the
/// default security group" (`DeleteVpc`) — so only `DeleteVpc` removes it.
pub fn cannot_delete<E>(group_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        CANNOT_DELETE,
        400,
        format!(
            "The security group '{group_id}' is the default group of its VPC \
             and cannot be deleted"
        ),
    )
}

/// Refuse a subnet CIDR block that overlaps one already in the VPC: HTTP 400
/// `InvalidSubnet.Conflict`.
pub fn invalid_subnet_conflict<E>(cidr_block: &str, subnet_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_SUBNET_CONFLICT,
        400,
        format!("The CIDR '{cidr_block}' conflicts with another subnet ('{subnet_id}')"),
    )
}

/// Refuse a subnet CIDR block that is not inside its VPC's own block: HTTP 400
/// `InvalidSubnet.Range`.
pub fn invalid_subnet_range<E>(cidr_block: &str, vpc_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_SUBNET_RANGE,
        400,
        format!("The CIDR '{cidr_block}' is not within the CIDR block of VPC '{vpc_id}'"),
    )
}

/// Refuse a create that would take the store past [`MAX_SUBNETS`]: HTTP 400
/// `SubnetLimitExceeded`.
pub fn subnet_limit_exceeded<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        SUBNET_LIMIT_EXCEEDED,
        400,
        format!("The maximum number of subnets has been reached ({MAX_SUBNETS})"),
    )
}

/// EC2's error code for a second security group with a name the VPC already holds: "You
/// can't have two security groups for the same VPC with the same name"
/// (`CreateSecurityGroup`).
pub const INVALID_GROUP_DUPLICATE: &str = "InvalidGroup.Duplicate";

/// Whether `id` is a well-formed security group id: `sg-` followed by 8 or 17 lowercase
/// hexadecimal digits — the short and the long id form, exactly as
/// [`instance_id_is_well_formed`] reads an instance id.
pub fn security_group_id_is_well_formed(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix(SECURITY_GROUP_ID_PREFIX) else {
        return false;
    };
    matches!(suffix.len(), 8 | 17) && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Whether `text` is within the alphabet a security group name and description are
/// documented to accept (see [`SECURITY_GROUP_PUNCTUATION`]).
pub fn is_security_group_text(text: &str) -> bool {
    text.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == ' ' || SECURITY_GROUP_PUNCTUATION.contains(c))
}

/// Refuse a second security group with a name the VPC already holds: HTTP 400
/// `InvalidGroup.Duplicate`.
pub fn invalid_group_duplicate<E>(group_name: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_GROUP_DUPLICATE,
        400,
        format!("The security group '{group_name}' already exists for VPC"),
    )
}

/// Refuse a create that named no VPC in an account that has no default VPC to fall back
/// on: HTTP 400 `VPCIdNotSpecified` — "The ID of the VPC. Required for a nondefault VPC"
/// (`CreateSecurityGroupRequest$VpcId`).
pub fn vpc_id_not_specified<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        VPC_ID_NOT_SPECIFIED,
        400,
        "No default VPC for this user, so the request must name a VpcId",
    )
}

/// EC2's error code for a request that left out a VPC id the account has no default VPC to
/// supply (EC2 API error codes, "VPCIdNotSpecified").
pub const VPC_ID_NOT_SPECIFIED: &str = "VPCIdNotSpecified";

/// Refuse a create that would take the store past [`MAX_SECURITY_GROUPS`]: HTTP 400
/// `SecurityGroupLimitExceeded`.
pub fn security_group_limit_exceeded<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        SECURITY_GROUP_LIMIT_EXCEEDED,
        400,
        format!("The maximum number of security groups has been reached ({MAX_SECURITY_GROUPS})"),
    )
}

/// The caller-chosen bytes a stored VPC retains: its tags, plus the strings an IPv6
/// association holds. The same quantity `CreateVpc` charged against the store's budget, so
/// [`ComAmazonawsEc2Amazonec2State::remove_vpc`] gives back exactly what was taken.
pub fn vpc_payload_bytes(vpc: &Vpc) -> usize {
    let ipv6_bytes: usize = vpc
        .ipv6_cidr_block_association_set
        .iter()
        .flatten()
        .map(|association| {
            association.ipv6_cidr_block.as_deref().map_or(0, str::len)
                + association.ipv6_pool.as_deref().map_or(0, str::len)
                + association.network_border_group.as_deref().map_or(0, str::len)
        })
        .sum();
    tag_list_bytes(vpc.tags.as_deref().unwrap_or_default()).saturating_add(ipv6_bytes)
}

/// The caller-chosen bytes a stored subnet retains: its tags, which is what
/// [`ComAmazonawsEc2Amazonec2State::insert_subnet`] charged.
pub fn subnet_payload_bytes(subnet: &Subnet) -> usize {
    tag_list_bytes(subnet.tags.as_deref().unwrap_or_default())
}

impl ComAmazonawsEc2Amazonec2State {
    /// The account's default VPC, by id — the VPC a request that names none is about:
    /// "If you don't specify a security group when you launch an instance, the instance is
    /// launched into the appropriate default security group" (`CreateSecurityGroup`), the
    /// group of that VPC.
    pub fn default_vpc_id(&self) -> Option<String> {
        self.vpcs
            .iter()
            .find(|(_, vpc)| vpc.is_default == Some(true))
            .map(|(vpc_id, _)| vpc_id.clone())
    }

    /// Whether `group_id` is the default security group of its VPC: the group named
    /// [`DEFAULT_SECURITY_GROUP_NAME`], which every VPC comes with and only `DeleteVpc`
    /// removes.
    pub fn is_default_security_group(&self, group_id: &str) -> bool {
        self.security_groups
            .get(group_id)
            .and_then(|group| group.group_name.as_deref())
            .is_some_and(|name| name == DEFAULT_SECURITY_GROUP_NAME)
    }

    /// The instances that still hold a place in `subnet_id`.
    ///
    /// "You must terminate all running instances in the subnet before you can delete the
    /// subnet" (`DeleteSubnet`) — an instance that has reached `terminated` holds nothing
    /// any more, so it is not a dependency; every other state is.
    pub fn instances_in_subnet(&self, subnet_id: &str) -> Vec<String> {
        let mut found: Vec<String> = self
            .instances
            .values()
            .filter(|instance| {
                instance.subnet_id.as_deref() == Some(subnet_id)
                    && instance.state != InstanceStateName::Terminated
            })
            .map(|instance| instance.instance_id.clone())
            .collect();
        found.sort();
        found
    }

    /// What still depends on the security group `group_id`, as the ids of the depending
    /// resources.
    ///
    /// "If you attempt to delete a security group that is associated with an instance or
    /// network interface, is referenced by another security group in the same VPC, or has a
    /// VPC association, the operation fails with `DependencyViolation`"
    /// (`DeleteSecurityGroup`): an instance that holds the group, and another group of the
    /// same VPC whose rules name it.
    pub fn security_group_dependencies(&self, group_id: &str) -> Vec<String> {
        let vpc_id = self
            .security_groups
            .get(group_id)
            .and_then(|group| group.vpc_id.clone());
        let mut found: Vec<String> = self
            .instances
            .values()
            .filter(|instance| {
                instance.state != InstanceStateName::Terminated
                    && instance
                        .security_groups
                        .iter()
                        .any(|(id, _)| id.as_deref() == Some(group_id))
            })
            .map(|instance| instance.instance_id.clone())
            .collect();
        for (other_id, other) in &self.security_groups {
            if other_id == group_id || other.vpc_id != vpc_id {
                continue;
            }
            let references = other
                .ip_permissions
                .iter()
                .chain(other.ip_permissions_egress.iter())
                .flatten()
                .flat_map(|permission| permission.user_id_group_pairs.iter().flatten())
                .any(|pair| pair.group_id.as_deref() == Some(group_id));
            if references {
                found.push(other_id.clone());
            }
        }
        found.sort();
        found
    }

    /// What still depends on the VPC `vpc_id`, as the ids of the depending resources.
    ///
    /// "You must detach or delete all gateways and resources that are associated with the
    /// VPC before you can delete it. For example, you must terminate all instances running
    /// in the VPC, delete all security groups associated with the VPC (except the default
    /// one) … When you delete the VPC, it deletes the default security group, network ACL,
    /// and route table for the VPC" (`DeleteVpc`). So the dependencies are this VPC's
    /// subnets, its security groups other than the default one, and the instances still
    /// running in it; the default group is NOT one, because the delete removes it.
    pub fn vpc_dependencies(&self, vpc_id: &str) -> Vec<String> {
        let mut found: Vec<String> = Vec::new();
        for (subnet_id, subnet) in &self.subnets {
            if subnet.vpc_id.as_deref() == Some(vpc_id) {
                found.push(subnet_id.clone());
            }
        }
        for (group_id, group) in &self.security_groups {
            if group.vpc_id.as_deref() == Some(vpc_id)
                && group.group_name.as_deref() != Some(DEFAULT_SECURITY_GROUP_NAME)
            {
                found.push(group_id.clone());
            }
        }
        for instance in self.instances.values() {
            if instance.state == InstanceStateName::Terminated {
                continue;
            }
            let in_vpc = instance.subnet_id.as_deref().is_some_and(|subnet_id| {
                self.subnets
                    .get(subnet_id)
                    .and_then(|subnet| subnet.vpc_id.as_deref())
                    == Some(vpc_id)
            });
            if in_vpc {
                found.push(instance.instance_id.clone());
            }
        }
        found.sort();
        found
    }

    /// Remove the subnet `subnet_id` and answer it as it was last reported, or `None` when
    /// the store holds no such subnet. The bytes the record held go back to the store's
    /// budget, and the `available` transition the clock may still owe it goes with it, so a
    /// later materialisation cannot resurrect a deleted subnet's state.
    pub fn remove_subnet(&mut self, subnet_id: &str) -> Option<Subnet> {
        let subnet = self.subnets.remove(subnet_id)?;
        self.subnet_available_at.remove(subnet_id);
        self.release_stored_bytes(subnet_payload_bytes(&subnet));
        Some(subnet)
    }

    /// Remove the security group `group_id` and answer it as it was last reported, or
    /// `None` when the store holds no such group.
    ///
    /// The bytes the record held go back to the store's budget on exactly the terms they
    /// were taken on: `CreateSecurityGroup` charged [`security_group_payload_bytes`] of the
    /// group it stored, while the default group of a VPC carries a name and description the
    /// SERVICE chose and was never charged for them — so releasing for it would walk
    /// `stored_bytes` below what the store actually holds and silently weaken
    /// [`MAX_STORED_BYTES`]. A caller cannot create a second group named
    /// [`DEFAULT_SECURITY_GROUP_NAME`] in a VPC (the name is already taken), so that name
    /// identifies the uncharged group exactly.
    pub fn remove_security_group(&mut self, group_id: &str) -> Option<SecurityGroup> {
        let was_default = self.is_default_security_group(group_id);
        let group = self.security_groups.remove(group_id)?;
        if !was_default {
            self.release_stored_bytes(security_group_payload_bytes(&group));
        }
        Some(group)
    }

    /// Remove the VPC `vpc_id` with the resources its delete takes with it, and answer it
    /// as it was last reported, or `None` when the store holds no such VPC.
    ///
    /// "When you delete the VPC, it deletes the default security group, network ACL, and
    /// route table for the VPC" (`DeleteVpc`), so the groups of this VPC go here rather
    /// than being left behind naming a VPC that no longer exists — the caller has already
    /// had to delete every other one ([`Self::vpc_dependencies`]). The bytes every removed
    /// record held go back to the store's budget, and the `available` transition the clock
    /// may still owe the VPC goes with it.
    pub fn remove_vpc(&mut self, vpc_id: &str) -> Option<Vpc> {
        let vpc = self.vpcs.remove(vpc_id)?;
        self.vpc_available_at.remove(vpc_id);
        // The attributes `insert_vpc` wrote for this VPC go with it as well: a VPC created
        // later must start from the creation defaults, never from what a
        // `ModifyVpcAttribute` left behind for a VPC that no longer exists.
        self.vpc_attributes.remove(vpc_id);
        self.release_stored_bytes(vpc_payload_bytes(&vpc));
        let groups: Vec<String> = self
            .security_groups
            .iter()
            .filter(|(_, group)| group.vpc_id.as_deref() == Some(vpc_id))
            .map(|(group_id, _)| group_id.clone())
            .collect();
        for group_id in groups {
            self.remove_security_group(&group_id);
        }
        Some(vpc)
    }
}

// ---------------------------------------------------------------------------
// BYOIP address ranges
// ---------------------------------------------------------------------------

/// `ByoipCidrState` of a range AWS has finished provisioning and will advertise on
/// request.
pub const BYOIP_CIDR_STATE_PROVISIONED: &str = "provisioned";

/// `ByoipCidrState` of a provisioned range the account asked AWS not to advertise
/// ("PubliclyAdvertisable ... Default: true", `ProvisionByoipCidrRequest`).
pub const BYOIP_CIDR_STATE_PROVISIONED_NOT_PUBLICLY_ADVERTISABLE: &str =
    "provisioned-not-publicly-advertisable";

/// `ByoipCidrState` of a range whose provisioning did not complete.
pub const BYOIP_CIDR_STATE_FAILED_PROVISION: &str = "failed-provision";

/// `ByoipCidrState` of a range whose release did not complete.
pub const BYOIP_CIDR_STATE_FAILED_DEPROVISION: &str = "failed-deprovision";

/// `ByoipCidrState` of a range whose release is under way.
pub const BYOIP_CIDR_STATE_PENDING_DEPROVISION: &str = "pending-deprovision";

/// `ByoipCidrState` of a range AWS is advertising to the internet.
pub const BYOIP_CIDR_STATE_ADVERTISED: &str = "advertised";

/// The `ByoipCidrState`s an address range can be RELEASED from.
///
/// "Before you can release an address range, you must stop advertising it and you must not
/// have any IP addresses allocated from its address range" (`DeprovisionByoipCidr`) — so a
/// range AWS is advertising, or one part way through a move the clock has not finished
/// (`pending-advertising`, `pending-withdrawal`, `pending-provision`,
/// `pending-deprovision`), is not releasable: the caller has to wait for or undo that
/// first. What is left are the states where the range is the account's and sitting still:
/// it finished provisioning (`provisioned`, or
/// `provisioned-not-publicly-advertisable` when the account asked for it not to be
/// advertised), or a provision or an earlier release of it failed and the range is the
/// caller's to clear (`failed-provision`, `failed-deprovision`). Every word here is one of
/// `ByoipCidrState`'s own values (`BYOIP_CIDR_STATE_VALUES` in `ops::types`).
pub const RELEASABLE_BYOIP_CIDR_STATES: &[&str] = &[
    BYOIP_CIDR_STATE_PROVISIONED,
    BYOIP_CIDR_STATE_PROVISIONED_NOT_PUBLICLY_ADVERTISABLE,
    BYOIP_CIDR_STATE_FAILED_PROVISION,
    BYOIP_CIDR_STATE_FAILED_DEPROVISION,
];

/// Can an address range in `state` be released? See [`RELEASABLE_BYOIP_CIDR_STATES`].
pub fn byoip_cidr_state_is_releasable(state: &str) -> bool {
    RELEASABLE_BYOIP_CIDR_STATES.contains(&state)
}

/// One BYOIP address range the account holds: the modelled [`ByoipCidr`] every operation
/// that reports one answers with, plus the release the clock owes it, if any.
///
/// The state word lives on the modelled shape rather than beside it, so a range reports
/// one state however it is read; only the release, which no member of [`ByoipCidr`]
/// carries, is stored next to it.
#[derive(Debug, Clone)]
pub struct ByoipCidrRecord {
    /// The range as the model shapes it.
    pub byoip_cidr: ByoipCidr,
    /// When the release of a `pending-deprovision` range ends, for a range a
    /// `DeprovisionByoipCidr` has started; `None` for a range nobody is releasing.
    ///
    /// Monotonic ([`Instant`]), like [`VolumeRecord::available_at`] and
    /// [`ComAmazonawsEc2Amazonec2State::vpc_available_at`], because it is only ever
    /// compared against the clock and never reported, so a wall-clock adjustment cannot
    /// resurrect a released range. WHAT the release ends as is NOT stored: it is decided at
    /// the due instant, in
    /// [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`], from the addresses
    /// the account holds out of the range then.
    pub released_at: Option<Instant>,
    /// The state move an ADVERTISEMENT owes the range, for a range `AdvertiseByoipCidr` left
    /// in `pending-advertising`; `None` for a range nothing is advertising.
    ///
    /// Beside [`Self::released_at`] rather than folded into it because the two owe different
    /// things: a release's outcome is decided at its due instant from the addresses the
    /// account holds then, while an advertisement's is known when it is asked for, so it is
    /// carried as the [`PendingStateMove`] every other record of this store uses.
    pub pending: Option<PendingStateMove>,
}

impl ByoipCidrRecord {
    /// A range sitting in `state`, with nothing owed to it by the clock.
    pub fn in_state(cidr: &str, state: &str) -> Self {
        Self {
            byoip_cidr: ByoipCidr {
                cidr: Some(cidr.to_string()),
                state: Some(state.to_string()),
                ..ByoipCidr::default()
            },
            released_at: None,
            pending: None,
        }
    }

    /// The range's current `ByoipCidrState`.
    pub fn state(&self) -> &str {
        self.byoip_cidr.state.as_deref().unwrap_or_default()
    }
}

/// The bytes one stored BYOIP address range holds.
///
/// The ONE definition of that charge, the way [`volume_recycle_bin_entry_payload_bytes`] is
/// for an entry of the Recycle Bin: the fixed [`STORED_RECORD_OVERHEAD_BYTES`] every stored
/// record costs — so a range carrying no optional text is NOT free against
/// [`MAX_STORED_BYTES`] — plus the caller-chosen text the record carries, which is what a
/// request can make large. [`ComAmazonawsEc2Amazonec2State::insert_byoip_cidr`] charges
/// exactly this and every path that drops a range gives back exactly this.
pub fn byoip_cidr_payload_bytes(record: &ByoipCidrRecord) -> usize {
    let range = &record.byoip_cidr;
    let asn_bytes = range
        .asn_associations
        .iter()
        .flatten()
        .map(|association| {
            association
                .asn
                .as_deref()
                .map_or(0, str::len)
                .saturating_add(association.cidr.as_deref().map_or(0, str::len))
                .saturating_add(association.status_message.as_deref().map_or(0, str::len))
                .saturating_add(association.state.as_deref().map_or(0, str::len))
        })
        .fold(0usize, usize::saturating_add);
    STORED_RECORD_OVERHEAD_BYTES
        .saturating_add(range.cidr.as_deref().map_or(0, str::len))
        .saturating_add(range.description.as_deref().map_or(0, str::len))
        .saturating_add(range.status_message.as_deref().map_or(0, str::len))
        .saturating_add(range.network_border_group.as_deref().map_or(0, str::len))
        .saturating_add(range.advertisement_type.as_deref().map_or(0, str::len))
        .saturating_add(range.pool_id.as_deref().map_or(0, str::len))
        .saturating_add(range.ipam_pool_id.as_deref().map_or(0, str::len))
        .saturating_add(asn_bytes)
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another BYOIP address range fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]).
    ///
    /// The same contract as [`Self::has_volume_recycle_bin_capacity`]: a writer asks this
    /// BEFORE provisioning anything and refuses the request with EC2's own quota code when
    /// it answers `false`, and [`Self::insert_byoip_cidr`] asks it again for itself, so no
    /// path can get past the ceiling by forgetting to. The ceiling is needed because the
    /// key is the range a REQUEST names, so without it a client could provision distinct
    /// ranges without end and grow this process's memory without bound — nothing in this
    /// module reclaims a stored range except the release the clock applies.
    pub fn has_byoip_cidr_capacity(&self) -> bool {
        self.byoip_cidrs.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Store `record` as the account's range for the `Cidr` it carries, replacing whatever
    /// range that CIDR named before, and answer whether it was stored.
    ///
    /// The key is the range in the CANONICAL form the store documents
    /// ([`canonical_byoip_cidr`]), taken here rather than trusted from the caller, so a
    /// writer that stores `192.0.2.5/24` cannot create a range no lookup can name. A record
    /// carrying no `Cidr`, or one that is not an address range at all, names no range the
    /// API can address — every BYOIP operation is addressed by `Cidr` — so there is no key
    /// to file it under and it is not stored.
    ///
    /// `false`, with NOTHING written, when there is no key to file it under, or when the
    /// range is a new one and the store is at [`MAX_STORED_RECORDS_PER_STORE`] or the bytes
    /// it would hold would cross [`MAX_STORED_BYTES`]. Both ceilings are asked before the
    /// first write, so a refused provision leaves the store exactly as it was.
    pub fn insert_byoip_cidr(&mut self, record: ByoipCidrRecord) -> bool {
        let Some(cidr) = record
            .byoip_cidr
            .cidr
            .as_deref()
            .and_then(|cidr| canonical_byoip_cidr(cidr).ok())
        else {
            return false;
        };
        let charge = byoip_cidr_payload_bytes(&record);
        // A range the store does not already hold is a NEW record, so it is the one that
        // has to fit inside both ceilings. Replacing a range held under the same key stores
        // no further record and gives back what the record it replaces was charged, so the
        // byte budget stays the sum of what the store actually holds.
        let refund = self.byoip_cidrs.get(&cidr).map(byoip_cidr_payload_bytes);
        if refund.is_none()
            && (!self.has_byoip_cidr_capacity() || self.would_exceed_stored_bytes(charge))
        {
            return false;
        }
        if let Some(refund) = refund {
            self.release_stored_bytes(refund);
        }
        self.add_stored_bytes(charge);
        self.byoip_cidrs.insert(cidr, record);
        true
    }

    /// Start releasing the range `cidr`: move it to `pending-deprovision` and record when
    /// the release ends, answering the range as the caller is told about it.
    ///
    /// The release is NOT applied here. "Releases the specified address range"
    /// (`DeprovisionByoipCidr`) is work AWS reports as under way — the range comes back in
    /// a non-terminal state — so it ends [`TRANSITION_DELAY`] later, through
    /// [`Self::materialize_due_transitions`], which is also where WHAT it ends as is
    /// decided; a caller that asks again immediately still sees the `pending-deprovision`
    /// range.
    pub fn begin_byoip_cidr_release(&mut self, cidr: &str) -> Option<ByoipCidr> {
        let record = self.byoip_cidrs.get_mut(cidr)?;
        record.byoip_cidr.state = Some(BYOIP_CIDR_STATE_PENDING_DEPROVISION.to_string());
        record.byoip_cidr.status_message = None;
        record.released_at = Some(Instant::now() + TRANSITION_DELAY);
        Some(record.byoip_cidr.clone())
    }

    /// The allocation ids of the account's Elastic IP addresses that come out of the
    /// address range `cidr`, in id order.
    ///
    /// "You must not have any IP addresses allocated from its address range"
    /// (`DeprovisionByoipCidr`) is a question about the addresses the account HOLDS, so it
    /// is answered from the address store rather than from a counter on the range.
    ///
    /// An Elastic IP address is IPv4 ([`Address::public_ip`] is a dotted quad), so an IPv6
    /// range holds none: [`ipv4_cidr_block_contains`] reads no bounds for an IPv6 `cidr`
    /// and answers `false`, which is why the IPv6 case needs no arm of its own here.
    pub fn addresses_in_byoip_cidr(&self, cidr: &str) -> Vec<String> {
        debug_assert!(
            cidr.contains('/'),
            "addresses_in_byoip_cidr takes an address range, got {cidr:?}"
        );
        self.addresses
            .iter()
            .filter(|(_, address)| {
                address.public_ip.as_deref().is_some_and(|public_ip| {
                    ipv4_cidr_block_contains(cidr, &format!("{public_ip}/32"))
                })
            })
            .map(|(allocation_id, _)| allocation_id.clone())
            .collect()
    }
}

/// The `StatusMessage` a release that failed because addresses are still allocated out of
/// the range carries: "Ensure that all EIPs from the range have been deallocated and try
/// again" (`ByoipCidrState`'s `failed-deprovision`), naming the allocations that held it so
/// the caller can clear them from this text alone.
///
/// Built from the store, never from a recording, and the ONE wording of that outcome.
pub fn byoip_cidr_release_failure(held: &[String]) -> String {
    format!(
        "Deprovisioning failed: {count} Elastic IP address(es) are still allocated from this \
         address range ({allocations}). Deallocate them and try again.",
        count = held.len(),
        allocations = held.join(", "),
    )
}

/// `value` as an address range in the canonical form this store keys its BYOIP ranges by:
/// an IPv4 or an IPv6 CIDR block with its host bits cleared.
///
/// BYOIP covers both families — "We recommend that you stop advertising the BYOIP CIDR from
/// other locations before you advertise it through Amazon" and the IPv6 notes on
/// `ProvisionByoipCidr` — so a range is whichever of the two it parses as, read by the same
/// canonicalisers the VPC and subnet paths use. Deliberately NO prefix-length rule: the
/// sizes AWS accepts for a range brought from a caller's own allocation are documented
/// nowhere the model carries, and a bound invented here would refuse ranges AWS accepts
/// (the same reason [`canonical_ipv6_cidr`] states none).
pub fn canonical_byoip_cidr(value: &str) -> Result<String, CidrProblem> {
    canonical_ipv4_cidr(value).or_else(|_| canonical_ipv6_cidr(value))
}

/// EC2's error code for an address range the request named that the account has not
/// brought to AWS, so there is no range to act on.
pub const INVALID_CIDR_NOT_FOUND: &str = "InvalidCidr.NotFound";

/// Refuse an address range this account has not provisioned: HTTP 400
/// `InvalidCidr.NotFound`.
///
/// The message names the range the request asked about, in EC2's own "the value you sent,
/// in parentheses" form, so the caller can tell which of several ranges was not found from
/// the text alone.
pub fn invalid_cidr_not_found<E>(cidr: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_CIDR_NOT_FOUND,
        400,
        format!("The CIDR ({cidr}) does not exist."),
    )
}

// The error code and refusal for a resource whose current state does not admit the request
// — "The resource is not in the correct state for the request" (EC2 API error codes) — are
// `INCORRECT_STATE` / `incorrect_state`, declared with the fast launch codes below;
// `DeprovisionByoipCidr`'s state refusal uses that one rather than a second copy.

// ---------------------------------------------------------------------------
// Capacity Blocks
// ---------------------------------------------------------------------------

/// A Capacity Block whose reserved window has not opened yet: the
/// `CapacityBlockResourceState` enum's `scheduled` member
/// (`smithy_query(query="shape",shape="CapacityBlockResourceState")`).
pub const CAPACITY_BLOCK_STATE_SCHEDULED: &str = "scheduled";

/// A Capacity Block whose reserved window is open: that enum's `active` member.
pub const CAPACITY_BLOCK_STATE_ACTIVE: &str = "active";

/// A Capacity Block whose reserved window has closed: that enum's `expired` member — "When
/// a Capacity Block expires, all instances in the Capacity Block are terminated"
/// (`CapacityBlock$EndDate`).
pub const CAPACITY_BLOCK_STATE_EXPIRED: &str = "expired";

/// The members of `CapacityBlockResourceState` that are NOT a fact of where the clock
/// stands in a block's reserved window, and so are stored on the record rather than derived
/// ([`CapacityBlockRecord::pinned_state`]): a block AWS could not deliver, one that was
/// cancelled, one that failed, and the two payment states a purchase can sit in.
pub const CAPACITY_BLOCK_PINNED_STATES: [&str; 5] = [
    "unavailable",
    "cancelled",
    "failed",
    "payment-pending",
    "payment-failed",
];

/// The id prefix a Capacity Block's id carries, in the `<prefix>-<hex>` form every EC2
/// resource id takes (see [`ComAmazonawsEc2Amazonec2State::next_id`]).
pub const CAPACITY_BLOCK_ID_PREFIX: &str = "cb";

/// The code EC2 answers for a Capacity Block id the account holds no block for, in the
/// `Invalid<Resource>Id.NotFound` family every other id of this service is refused with
/// ([`INVALID_LAUNCH_TEMPLATE_ID_NOT_FOUND`], [`INVALID_SUBNET_ID_NOT_FOUND`]).
pub const INVALID_CAPACITY_BLOCK_ID_NOT_FOUND: &str = "InvalidCapacityBlockId.NotFound";

/// How many Capacity Blocks this emulator stores before a writer must refuse.
///
/// CHOSEN, like [`MAX_STORED_VOLUMES`] and for the same reason: nothing in this emulator
/// reclaims a stored block, so the store needs a ceiling that is not the process's memory.
/// A count ceiling is no bound on the caller-chosen bytes each record holds, so those are
/// charged against [`MAX_STORED_BYTES`] too ([`capacity_block_payload_bytes`]).
pub const MAX_STORED_CAPACITY_BLOCKS: usize = 4096;

/// The caller-chosen bytes one stored Capacity Block holds: its tags and the Capacity
/// Reservation ids it holds its capacity as. Everything else on the record — the id, the
/// zone, the window instants, the state — is derived by this module or bounded by the
/// model, so it is fixed-size.
///
/// The ONE definition of that charge, the way [`host_payload_bytes`] and
/// [`address_payload_bytes`] are for their own stores:
/// [`ComAmazonawsEc2Amazonec2State::insert_capacity_block`] charges exactly it, and a
/// removal path must give back the same number.
pub fn capacity_block_payload_bytes(block: &CapacityBlock) -> usize {
    let reservation_bytes: usize = block
        .capacity_reservation_ids
        .iter()
        .flatten()
        .map(String::len)
        .sum();
    tag_list_bytes(block.tags.as_deref().unwrap_or_default()).saturating_add(reservation_bytes)
}

/// One stored Capacity Block: the modelled [`CapacityBlock`] every operation that reports
/// one answers with, plus the state a writer pinned it to.
///
/// The view holds no `State`: a block's `scheduled` -> `active` -> `expired` progression is
/// a fact of where the wall clock stands in the window its `StartDate` and `EndDate`
/// delimit, so [`Self::state_at`] derives it rather than letting two writers store two
/// different words for it. The clock is the WALL clock here, not the monotonic one a
/// [`PendingTransition`] is due on, because these two instants are themselves reported to
/// the caller.
#[derive(Debug, Clone)]
pub struct CapacityBlockRecord {
    /// The block as the model shapes it, with `State` left for [`Self::view_at`] to fill.
    pub block: CapacityBlock,
    /// The state of a block the clock does not move: one of
    /// [`CAPACITY_BLOCK_PINNED_STATES`], or `None` for a block whose state is read off its
    /// reserved window. A `&'static str` so it can only ever be a member of the modelled
    /// enum, never text taken from a request.
    pub pinned_state: Option<&'static str>,
}

impl CapacityBlockRecord {
    /// This block as of `now`: the stored view with the `State` the clock has reached. A
    /// read observes the move the clock already owed without writing anything.
    pub fn view_at(&self, now: SystemTime) -> CapacityBlock {
        let mut block = self.block.clone();
        block.state = Some(self.state_at(now).to_string());
        block
    }

    /// The block's `CapacityBlockResourceState` at `now`.
    ///
    /// A pinned state wins: no passage of time moves a cancelled or unpaid block into its
    /// reserved window. Otherwise the window delimits the three clock-driven states, a
    /// block being `scheduled` before its `StartDate`, `active` from it, and `expired` from
    /// its `EndDate`. A date the record does not carry does not bound the window on that
    /// side, so a block with neither is `active`.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if let Some(pinned) = self.pinned_state {
            return pinned;
        }
        let now = timestamp_key(system_time_to_date_time(now));
        if self.block.start_date.map(timestamp_key).is_some_and(|start| now < start) {
            return CAPACITY_BLOCK_STATE_SCHEDULED;
        }
        if self.block.end_date.map(timestamp_key).is_some_and(|end| now >= end) {
            return CAPACITY_BLOCK_STATE_EXPIRED;
        }
        CAPACITY_BLOCK_STATE_ACTIVE
    }
}

/// A timestamp as a totally ordered key: the modelled timestamp type carries no `Ord`, and
/// comparing the two instants of a window through `f64` seconds would lose precision the
/// `MillisecondDateTime` the model uses for them keeps.
fn timestamp_key(at: ::aws_smithy_types::DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos())
}

/// What one stored Capacity Block is created with, for
/// [`ComAmazonawsEc2Amazonec2State::insert_capacity_block`].
///
/// The instants are [`SystemTime`]s rather than modelled timestamps because every writer
/// computes them from the clock it already holds; the store converts them once.
#[derive(Debug, Clone)]
pub struct CapacityBlockSpec {
    /// "The EC2 UltraServer type of the Capacity Block" (`CapacityBlock$UltraserverType`).
    pub ultraserver_type: Option<String>,
    /// The Availability Zone the block's capacity sits in.
    pub availability_zone: String,
    /// That zone's id.
    pub availability_zone_id: String,
    /// The Capacity Reservations the block holds its capacity as.
    pub capacity_reservation_ids: Vec<String>,
    /// When the block's reserved window opens.
    pub start_date: SystemTime,
    /// When it closes, after which the block is `expired`.
    pub end_date: SystemTime,
    /// When the block was created.
    pub create_date: SystemTime,
    /// A state no clock drives, or `None` for a block whose state is its window's — see
    /// [`CapacityBlockRecord::pinned_state`].
    pub pinned_state: Option<&'static str>,
    /// "The tags assigned to the Capacity Block" (`CapacityBlock$Tags`).
    pub tags: Vec<Tag>,
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another `count` Capacity Blocks fit inside the store's ceiling
    /// ([`MAX_STORED_CAPACITY_BLOCKS`]).
    ///
    /// A writer must ask this BEFORE inserting and refuse when it answers `false`, the way
    /// [`Self::has_vpc_capacity`] and [`Self::has_dedicated_host_capacity`] gate their own
    /// stores: nothing in this emulator reclaims a stored block, so without a count ceiling
    /// repeated writes would grow memory without bound.
    pub fn has_capacity_block_capacity(&self, count: usize) -> bool {
        self.capacity_blocks.len().saturating_add(count) <= MAX_STORED_CAPACITY_BLOCKS
    }

    /// Store one Capacity Block and answer the `CapacityBlockId` it was given.
    ///
    /// The one place a Capacity Block enters this store, so every writer of one — and
    /// `DescribeCapacityBlocks`, which reports what this wrote — agrees on the id form, the
    /// stored shape and which members a block carries. The caller-chosen bytes the record
    /// retains are charged here, so no path can store them uncharged.
    pub fn insert_capacity_block(&mut self, spec: CapacityBlockSpec) -> String {
        let capacity_block_id = self.next_id(CAPACITY_BLOCK_ID_PREFIX);
        let block = CapacityBlock {
            capacity_block_id: Some(capacity_block_id.clone()),
            ultraserver_type: spec.ultraserver_type,
            availability_zone: Some(spec.availability_zone),
            availability_zone_id: Some(spec.availability_zone_id),
            capacity_reservation_ids: Some(spec.capacity_reservation_ids),
            start_date: Some(system_time_to_date_time(spec.start_date)),
            end_date: Some(system_time_to_date_time(spec.end_date)),
            create_date: Some(system_time_to_date_time(spec.create_date)),
            // Derived on read from the window above, so nothing is stored here; see
            // [`CapacityBlockRecord`].
            state: None,
            tags: Some(spec.tags),
        };
        self.add_stored_bytes(capacity_block_payload_bytes(&block));
        self.capacity_blocks.insert(
            capacity_block_id.clone(),
            CapacityBlockRecord {
                block,
                pinned_state: spec.pinned_state,
            },
        );
        capacity_block_id
    }
}

/// Refuse a request that named a Capacity Block id no block of the account has: HTTP 400
/// `InvalidCapacityBlockId.NotFound`.
///
/// The model publishes no pattern for `CapacityBlockId`
/// (`smithy_query(query="shape",shape="CapacityBlockId")`), so there is no shape to call
/// malformed ahead of the lookup the way a volume id has one: an id this store cannot
/// resolve is reported not found whatever it looks like.
pub fn invalid_capacity_block_id_not_found<E>(
    capacity_block_id: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_CAPACITY_BLOCK_ID_NOT_FOUND,
        400,
        format!("The Capacity Block ID '{capacity_block_id}' does not exist"),
    )
}

/// Refuse a Capacity Block id the request named that this store holds no block for.
pub fn require_known_capacity_block<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    capacity_block_id: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    if store.capacity_blocks.contains_key(capacity_block_id) {
        return Ok(());
    }
    Err(invalid_capacity_block_id_not_found(capacity_block_id))
}

// =============================================================================
// Capacity Block offerings
// =============================================================================

/// The currency a Capacity Block offering is priced in: the only member of the modelled
/// `CurrencyCodeValues` enum (`smithy_query(query="shape",shape="CurrencyCodeValues")`), so
/// it is the one value `CapacityBlockOffering$CurrencyCode` can carry.
pub const CAPACITY_BLOCK_OFFERING_CURRENCY_CODE: &str = "USD";

/// "The tenancy of the Capacity Block" (`CapacityBlockOffering$Tenancy`): the `default`
/// member of the modelled `CapacityReservationTenancy` enum
/// (`smithy_query(query="shape",shape="CapacityReservationTenancy")`). The other member,
/// `dedicated`, is single-tenant hardware a caller asks for when it creates a Capacity
/// Reservation, and nothing in a Capacity Block offering search asks for it.
pub const CAPACITY_BLOCK_OFFERING_TENANCY: &str = "default";

/// The id prefix a Capacity Block offering id carries, in the `<prefix>-<payload>` form
/// every EC2 resource id takes; `cbo` is the prefix AWS gives an offering id of this
/// operation, beside [`CAPACITY_BLOCK_ID_PREFIX`] for the block a purchase mints.
pub const CAPACITY_BLOCK_OFFERING_ID_PREFIX: &str = "cbo";

/// The marker an offering id carries ahead of its checksum and terms, so an id a caller
/// assembled is detectable rather than read as terms — the same arrangement
/// [`PAGE_TOKEN_VERSION`] gives a pagination token.
const CAPACITY_BLOCK_OFFERING_ID_VERSION: &str = "ec2-cbo-1";

/// What one reserved unit of a Capacity Block — one instance, or one EC2 UltraServer —
/// costs for one hour, in cents, as [`capacity_block_upfront_fee`] prices an offering.
///
/// CHOSEN, like [`MAX_INSTANCES_PER_RUN`]: "The total price to be paid up front"
/// (`CapacityBlockOffering$UpfrontFee`) is a function of AWS's price list, which neither the
/// model nor a request carries, so this emulator prices every reserved unit the same rather
/// than inventing a per-instance-type price list. What a caller can rely on is the RULE —
/// the fee is the reserved units times the reserved hours times one rate — not the rate.
pub const CAPACITY_BLOCK_FEE_CENTS_PER_UNIT_HOUR: u64 = 1_000;

/// "The total price to be paid up front" for `reserved_units` units held for
/// `duration_hours` hours, as `CapacityBlockOffering$UpfrontFee` carries it: a decimal
/// amount in [`CAPACITY_BLOCK_OFFERING_CURRENCY_CODE`], with the two fractional digits a
/// currency amount is written with.
///
/// The ONE definition of that charge, so every operation that reports an offering prices
/// one window the same way.
pub fn capacity_block_upfront_fee(reserved_units: u64, duration_hours: u64) -> String {
    let cents = reserved_units
        .saturating_mul(duration_hours)
        .saturating_mul(CAPACITY_BLOCK_FEE_CENTS_PER_UNIT_HOUR);
    format!("{}.{:02}", cents / 100, cents % 100)
}

/// What one Capacity Block offering reserves: the terms a purchase of it would turn into a
/// stored Capacity Block.
///
/// An offering is not a stored resource — see the module docs — so the terms travel INSIDE
/// the offering id ([`encode_capacity_block_offering_id`]) rather than in a map keyed by it.
/// That is what lets `DescribeCapacityBlockOfferings` report offerings without writing
/// anything, while a purchase can still resolve the id a caller hands back to the window,
/// zone and counts the offering was reported with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapacityBlockOfferingTerms {
    /// "The instance type of the Capacity Block offering"
    /// (`CapacityBlockOffering$InstanceType`), or `None` for an offering of EC2
    /// UltraServers, which names an UltraServer type instead.
    pub instance_type: Option<String>,
    /// "The number of instances in the Capacity Block offering"
    /// (`CapacityBlockOffering$InstanceCount`).
    pub instance_count: Option<i32>,
    /// "The EC2 UltraServer type of the Capacity Block offering"
    /// (`CapacityBlockOffering$UltraserverType`), or `None` for an offering of plain
    /// instances.
    pub ultraserver_type: Option<String>,
    /// "The number of EC2 UltraServers in the offering"
    /// (`CapacityBlockOffering$UltraserverCount`).
    pub ultraserver_count: Option<i32>,
    /// The Availability Zone the reserved capacity sits in.
    pub availability_zone: String,
    /// That zone's id. The modelled `CapacityBlockOffering` carries no member for it, but
    /// the [`CapacityBlock`] a purchase mints does, so the offering's terms keep it.
    pub availability_zone_id: String,
    /// When the reserved window opens, in seconds since the Unix epoch.
    pub start_date_secs: i64,
    /// When it closes, in seconds since the Unix epoch.
    pub end_date_secs: i64,
}

/// The `CapacityBlockOfferingId` for `terms`: `cbo-<hex>` over the terms, this service's
/// offering-id marker and a checksum.
///
/// Deriving the id FROM the terms is what makes it stable — two searches that find the same
/// window in the same zone for the same counts report the same id, so a read changes nothing
/// a later caller sees — and resolvable, since [`decode_capacity_block_offering_id`] reads
/// the terms back out without a store. The checksum is the same non-secret one a pagination
/// token carries ([`page_token_checksum`]), for the same reason: it tells an id this service
/// minted from one a caller assembled, nothing more.
pub fn encode_capacity_block_offering_id(terms: &CapacityBlockOfferingTerms) -> String {
    let payload = capacity_block_offering_payload(terms);
    let checksum = page_token_checksum(&payload);
    let body = format!("{CAPACITY_BLOCK_OFFERING_ID_VERSION}:{checksum:016x}:{payload}");
    format!(
        "{CAPACITY_BLOCK_OFFERING_ID_PREFIX}-{}",
        ::hex::encode(body.as_bytes())
    )
}

/// The terms an id minted by [`encode_capacity_block_offering_id`] carries; `None` for
/// anything else — an id that does not carry this service's prefix or marker, is not hex,
/// whose checksum does not match its terms, or whose terms are not the fields an offering
/// has.
pub fn decode_capacity_block_offering_id(offering_id: &str) -> Option<CapacityBlockOfferingTerms> {
    let hexed = offering_id
        .strip_prefix(CAPACITY_BLOCK_OFFERING_ID_PREFIX)
        .and_then(|rest| rest.strip_prefix('-'))?;
    let decoded = String::from_utf8(::hex::decode(hexed).ok()?).ok()?;
    let marked = decoded
        .strip_prefix(CAPACITY_BLOCK_OFFERING_ID_VERSION)
        .and_then(|rest| rest.strip_prefix(':'))?;
    let (checksum, payload) = marked.split_once(':')?;
    if checksum != format!("{:016x}", page_token_checksum(payload)) {
        return None;
    }
    // Every text field is itself hex, so no byte a request chose can carry the separator and
    // shift the fields the service derived; the field count is therefore exact.
    let fields: Vec<&str> = payload
        .split(CAPACITY_BLOCK_OFFERING_FIELD_SEPARATOR)
        .collect();
    if fields.len() != CAPACITY_BLOCK_OFFERING_FIELDS {
        return None;
    }
    let terms = CapacityBlockOfferingTerms {
        start_date_secs: fields[0].parse().ok()?,
        end_date_secs: fields[1].parse().ok()?,
        instance_count: optional_count_field(fields[2])?,
        ultraserver_count: optional_count_field(fields[3])?,
        availability_zone: hex_field(fields[4])?,
        availability_zone_id: hex_field(fields[5])?,
        instance_type: optional_hex_field(fields[6])?,
        ultraserver_type: optional_hex_field(fields[7])?,
    };
    Some(terms)
}

/// How many fields an offering id's payload carries.
const CAPACITY_BLOCK_OFFERING_FIELDS: usize = 8;

/// What separates them. Every text field is hex-encoded inside the payload, so no field can
/// carry the separator whatever text it holds — see [`decode_capacity_block_offering_id`].
const CAPACITY_BLOCK_OFFERING_FIELD_SEPARATOR: char = '|';

/// The terms of one offering as the text its id carries them as: the instants and counts as
/// decimal, every text field as hex, so a field is always separator-free.
fn capacity_block_offering_payload(terms: &CapacityBlockOfferingTerms) -> String {
    let separator = CAPACITY_BLOCK_OFFERING_FIELD_SEPARATOR;
    let count = |value: Option<i32>| value.map(|count| count.to_string()).unwrap_or_default();
    let text = |value: Option<&str>| ::hex::encode(value.unwrap_or_default().as_bytes());
    format!(
        "{}{separator}{}{separator}{}{separator}{}{separator}{}{separator}{}{separator}{}{separator}{}",
        terms.start_date_secs,
        terms.end_date_secs,
        count(terms.instance_count),
        count(terms.ultraserver_count),
        text(Some(terms.availability_zone.as_str())),
        text(Some(terms.availability_zone_id.as_str())),
        text(terms.instance_type.as_deref()),
        text(terms.ultraserver_type.as_deref()),
    )
}

/// One count field of an offering id: an empty field is a member the offering does not
/// carry, anything that is not a number at all makes the whole id unreadable.
fn optional_count_field(field: &str) -> Option<Option<i32>> {
    if field.is_empty() {
        return Some(None);
    }
    field.parse().ok().map(Some)
}

/// One text field of an offering id: the text its hex carries. A field that is not hex, or
/// not UTF-8 under it, makes the whole id unreadable.
fn hex_field(field: &str) -> Option<String> {
    String::from_utf8(::hex::decode(field).ok()?).ok()
}

/// One optional text field of an offering id: an empty field is a member the offering does
/// not carry. The outer `None` is an unreadable field, the inner one an absent member.
fn optional_hex_field(field: &str) -> Option<Option<String>> {
    if field.is_empty() {
        return Some(None);
    }
    hex_field(field).map(Some)
}

// =============================================================================
// Windows fast launch
// =============================================================================

/// `FastLaunchStateCode`'s `enabling`: Amazon EC2 "launches an instance and runs through
/// Sysprep steps … then it creates a set of reserved snapshots" (`EnableFastLaunch`), and
/// the AMI cannot launch from them yet
/// (`smithy_query(query="shape",shape="FastLaunchStateCode")`).
pub const FAST_LAUNCH_STATE_ENABLING: &str = "enabling";

/// `FastLaunchStateCode`'s `enabled`: the pre-provisioned snapshots are in place.
pub const FAST_LAUNCH_STATE_ENABLED: &str = "enabled";

/// `FastLaunchStateCode`'s `disabling`: a caller has asked to "discontinue Windows fast
/// launch for a Windows AMI, and clean up existing pre-provisioned snapshots"
/// (`DisableFastLaunch`), and that cleanup is not finished.
pub const FAST_LAUNCH_STATE_DISABLING: &str = "disabling";

/// The one member of `FastLaunchResourceType`, and "the default value" when a request
/// names none (`EnableFastLaunchRequest$ResourceType`).
pub const FAST_LAUNCH_RESOURCE_TYPE_SNAPSHOT: &str = "snapshot";

/// "Value must be `6` or greater" (`EnableFastLaunchRequest$MaxParallelLaunches`).
///
/// Also what a request that names no maximum gets: every operation that reports a
/// configuration reports a `MaxParallelLaunches`, and the floor the member documents is the
/// only value the service has been told anything about.
pub const MIN_FAST_LAUNCH_MAX_PARALLEL_LAUNCHES: i32 = 6;

/// The smallest `TargetResourceCount` a configuration may ask for: "the number of
/// pre-provisioned snapshots to keep on hand for a Windows fast launch enabled AMI"
/// (`FastLaunchSnapshotConfigurationRequest$TargetResourceCount`) is a count of snapshots
/// kept on hand, so a configuration that keeps any keeps at least one. The model declares
/// no `@range` for the member, so a handler is the only place the floor can be enforced.
pub const MIN_FAST_LAUNCH_TARGET_RESOURCE_COUNT: i32 = 1;

/// The code EC2 answers when the resource a request names exists but is not in a state
/// that admits the request.
pub const INCORRECT_STATE: &str = "IncorrectState";

/// The code EC2 answers when a request is invalid in a way no member-specific code of its
/// own covers.
pub const INVALID_REQUEST: &str = "InvalidRequest";

/// The code EC2 answers for a launch template version the named template does not hold.
pub const INVALID_LAUNCH_TEMPLATE_ID_VERSION_NOT_FOUND: &str =
    "InvalidLaunchTemplateId.VersionNotFound";

/// The `Version` value that names a launch template's latest version: "valid values are
/// $Latest, $Default, and numbers"
/// (`DescribeLaunchTemplateVersionsRequest$Versions`).
pub const LAUNCH_TEMPLATE_VERSION_LATEST: &str = "$Latest";

/// The `Version` value that names a launch template's default version, from the same
/// sentence as [`LAUNCH_TEMPLATE_VERSION_LATEST`].
pub const LAUNCH_TEMPLATE_VERSION_DEFAULT: &str = "$Default";

/// Refuse a request the service answers no member-specific code for: HTTP 400
/// `InvalidRequest`.
pub fn invalid_request<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(INVALID_REQUEST, 400, message)
}

/// Refuse a request whose resource exists but is not in a state that admits it: HTTP 400
/// `IncorrectState`.
pub fn incorrect_state<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(INCORRECT_STATE, 400, message)
}

/// What the clock owes a stored Windows fast launch configuration next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FastLaunchMove {
    /// `enabling` -> `enabled`: the reserved snapshots an enable started are in place.
    Enabled,
    /// `disabling` -> reported no longer: the cleanup a disable started is finished, so
    /// the image is not a fast launch enabled image any more.
    Discontinued,
}

/// A move the clock owes one stored resource, with the instants it falls due on.
///
/// ONE definition for every resource kind whose next state is owed to the clock rather than
/// to a caller: the `T` is what that kind becomes ([`FastLaunchMove`] for a Windows fast
/// launch configuration, a `&'static str` catalog word for a VPC BPA exclusion). An
/// instance's [`PendingTransition`] is deliberately NOT one of these — it carries no
/// wall-clock instant, an instance reporting no transition time.
#[derive(Debug, Clone)]
pub struct ClockTransition<T> {
    /// What the resource becomes.
    pub to: T,
    /// The instant from which the move is due. Monotonic ([`Instant`]), because it is only
    /// ever compared against the clock and never reported, so a wall-clock adjustment
    /// cannot move a resource back to the state it is leaving.
    pub due_at: Instant,
    /// The wall-clock instant the move falls due, which the resource reports as the time it
    /// reached the new state once the move has been applied — the state changed when the
    /// move fell due, not when somebody next asked about it.
    pub at: SystemTime,
}

impl<T> ClockTransition<T> {
    /// A move to `to`, due [`TRANSITION_DELAY`] from now: the span this crate gives every
    /// clock-owed transition, recorded rather than slept for.
    pub fn starting_now(to: T) -> Self {
        Self {
            to,
            due_at: Instant::now() + TRANSITION_DELAY,
            at: SystemTime::now() + TRANSITION_DELAY,
        }
    }
}

/// The move the clock owes one Windows fast launch configuration: a [`ClockTransition`] to
/// the next [`FastLaunchMove`], reported as the configuration's `StateTransitionTime`.
pub type FastLaunchTransition = ClockTransition<FastLaunchMove>;

/// One image's Windows fast launch configuration: the modelled
/// [`DescribeFastLaunchImagesSuccessItem`] every operation that reports one answers with,
/// plus the move the clock owes it.
///
/// `EnableFastLaunch`, `DisableFastLaunch` and `DescribeFastLaunchImages` answer with the
/// same member set, so the one stored item is what all three report and they cannot
/// describe one configuration differently.
#[derive(Debug, Clone)]
pub struct FastLaunchRecord {
    /// The configuration as the model shapes it, carrying the state reached so far.
    pub view: DescribeFastLaunchImagesSuccessItem,
    /// The move the clock owes this configuration, while one is owed.
    pub pending: Option<FastLaunchTransition>,
}

impl FastLaunchRecord {
    /// The configuration's `FastLaunchStateCode` as it was last reported.
    pub fn state(&self) -> &str {
        self.view.state.as_deref().unwrap_or_default()
    }

    /// This configuration as `EnableFastLaunch` reports it.
    pub fn enable_response(&self) -> EnableFastLaunchOutput {
        let view = self.view.clone();
        EnableFastLaunchOutput {
            image_id: view.image_id,
            resource_type: view.resource_type,
            snapshot_configuration: view.snapshot_configuration,
            launch_template: view.launch_template,
            max_parallel_launches: view.max_parallel_launches,
            owner_id: view.owner_id,
            state: view.state,
            state_transition_reason: view.state_transition_reason,
            state_transition_time: view.state_transition_time,
        }
    }

    /// This configuration as `DisableFastLaunch` reports it.
    pub fn disable_response(&self) -> DisableFastLaunchOutput {
        let view = self.view.clone();
        DisableFastLaunchOutput {
            image_id: view.image_id,
            resource_type: view.resource_type,
            snapshot_configuration: view.snapshot_configuration,
            launch_template: view.launch_template,
            max_parallel_launches: view.max_parallel_launches,
            owner_id: view.owner_id,
            state: view.state,
            state_transition_reason: view.state_transition_reason,
            state_transition_time: view.state_transition_time,
        }
    }
}

/// Can the calling account see `image` at all?
///
/// "The images available to you include public images, private images that you own, and
/// private images owned by other Amazon Web Services accounts for which you have explicit
/// launch permissions" (`DescribeImages`). Nothing in this slice grants an explicit launch
/// permission — `ModifyImageAttribute` is still a scaffold — so the images available are
/// the public ones and the account's own, and a private image of another account is
/// invisible however a request asks for it.
pub fn image_is_visible(image: &Image, account: &str) -> bool {
    image.public.unwrap_or(false) || image.owner_id.as_deref() == Some(account)
}

/// Can the calling account see the image `image_id` at all — does the store hold an image
/// under that id that [`image_is_visible`] admits?
pub fn account_can_see_image(store: &ComAmazonawsEc2Amazonec2State, image_id: &str) -> bool {
    let account = ::engine::emulator_account_id();
    store
        .images
        .get(image_id)
        .is_some_and(|image| image_is_visible(image, account))
}

/// Refuse an image id a request named that the calling account cannot resolve: something
/// that is not an AMI id at all is malformed, and a well-formed id the account can see no
/// image for does not exist — the order EC2 answers the two in.
pub fn require_visible_image<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    image_id: &str,
) -> Result<(), Scaffolded<E>> {
    if !image_id_is_well_formed(image_id) {
        return Err(invalid_ami_id_malformed(image_id));
    }
    if !account_can_see_image(store, image_id) {
        return Err(invalid_ami_id_not_found(image_id));
    }
    Ok(())
}

/// Refuse an image id a Windows fast launch change names that cannot carry one.
///
/// Either the account cannot resolve it ([`require_visible_image`]), or the image is not a
/// Windows AMI: Windows fast launch is a setting of "Windows AMIs that you own or that have
/// been shared with you" (`EnableFastLaunch`), and "this value is set to `windows` for
/// Windows AMIs; otherwise, it is blank" (`Image$Platform`) is what says which an image is.
/// The images this account may change are therefore its own and the public ones — the same
/// set [`image_is_visible`] answers, no image here carrying an explicit launch permission.
pub fn require_windows_image<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    image_id: &str,
) -> Result<(), Scaffolded<E>> {
    require_visible_image(store, image_id)?;
    let is_windows = store
        .images
        .get(image_id)
        .and_then(|image| image.platform.as_deref())
        == Some(IMAGE_PLATFORM_WINDOWS);
    if !is_windows {
        return Err(invalid_parameter_value(format!(
            "Invalid value '{image_id}' for imageId. \
             Windows fast launch is supported for Windows AMIs only"
        )));
    }
    Ok(())
}

/// Resolve the launch template version a request names against the template that holds it.
///
/// "Valid values are $Latest, $Default, and numbers"
/// (`DescribeLaunchTemplateVersionsRequest$Versions`): an alias resolves to the number the
/// template records for it, and a number has to be a version the template holds — a
/// version that does not exist is `InvalidLaunchTemplateId.VersionNotFound`, while
/// something that is neither an alias nor a number is a bad parameter value.
pub fn resolve_launch_template_version<E>(
    record: &LaunchTemplateRecord,
    version: &str,
) -> Result<i64, Scaffolded<E>> {
    let numbered = match version {
        LAUNCH_TEMPLATE_VERSION_LATEST => record.template.latest_version_number,
        LAUNCH_TEMPLATE_VERSION_DEFAULT => record.template.default_version_number,
        other => match other.parse::<i64>() {
            Ok(number) => Some(number),
            Err(_) => {
                return Err(invalid_parameter_value(format!(
                    "Invalid value '{version}' for version. Valid values are \
                     {LAUNCH_TEMPLATE_VERSION_LATEST}, {LAUNCH_TEMPLATE_VERSION_DEFAULT}, \
                     and version numbers"
                )));
            }
        },
    };
    numbered
        .filter(|number| record.versions.contains_key(number))
        .ok_or_else(|| {
            Scaffolded::service_error(
                INVALID_LAUNCH_TEMPLATE_ID_VERSION_NOT_FOUND,
                400,
                format!(
                    "Launch template version '{version}' does not exist for launch \
                     template '{}'",
                    record.launch_template_id()
                ),
            )
        })
}

// =============================================================================
// Elastic IP addresses
// =============================================================================

/// The one network an Elastic IP address is allocated into today: `DomainType`'s `vpc`
/// ("The network (`vpc`)" — `AllocateAddressRequest$Domain`, and the same sentence on
/// `Address$Domain`).
pub const ADDRESS_DOMAIN_VPC: &str = "vpc";

/// `DomainType`'s other member, the EC2-Classic network
/// (`smithy_query(query="shape",shape="DomainType")`). The model documents only `vpc` as
/// the network an address is allocated into, so this value names the network a request
/// can no longer ask for.
pub const ADDRESS_DOMAIN_STANDARD: &str = "standard";

/// The one resource type `AllocateAddress`'s `TagSpecifications` can name: "The tags to
/// assign to the Elastic IP address" (`AllocateAddressRequest$TagSpecifications`), which
/// is the `elastic-ip` member of `ResourceType`.
pub const ADDRESS_TAGGABLE_RESOURCE: &str = "elastic-ip";

/// The address pool an Amazon-provided Elastic IP address comes from: "Amazon's pool of
/// IPv4 addresses" (`AllocateAddress`). An address out of that pool carries no
/// `PublicIpv4Pool` id — the member is "The ID of an address pool that you own" — so this
/// is the value the pool members report as, and the describe filter compares against
/// nothing for such an address.
pub const AMAZON_ADDRESS_POOL: &str = "amazon";

/// How many Elastic IP addresses this emulator stores before `AllocateAddress` is
/// refused.
///
/// CHOSEN, like [`MAX_STORED_VOLUMES`] and for the same reason: nothing in this emulator
/// reclaims a stored address, so the store needs a ceiling that is not the process's
/// memory. It is well above the published per-Region Elastic IP quota (5 by default,
/// which is adjustable), so a test that allocates more than a real account's default
/// quota is not refused for a reason this emulator cannot have.
pub const MAX_STORED_ADDRESSES: usize = 4096;

/// How many candidate addresses [`ComAmazonawsEc2Amazonec2State::mint_elastic_ip`] probes
/// before giving up, so the search cannot spin when the block it draws from is full.
pub const MAX_ELASTIC_IP_PROBES: u32 = 4096;

/// `AddressLimitExceeded` (HTTP 400): the allocation would take the account past the
/// number of Elastic IP addresses it may hold.
pub const ADDRESS_LIMIT_EXCEEDED: &str = "AddressLimitExceeded";

/// `InvalidAddress.NotFound` (HTTP 400): the request named an Elastic IP address this
/// account holds no allocation for.
pub const INVALID_ADDRESS_NOT_FOUND: &str = "InvalidAddress.NotFound";

/// `InvalidAllocationID.NotFound` (HTTP 400): the request named an allocation id this
/// account holds no address for.
pub const INVALID_ALLOCATION_ID_NOT_FOUND: &str = "InvalidAllocationID.NotFound";

/// `InvalidAllocationID.Malformed` (HTTP 400): the request named something that is not an
/// allocation id at all, so there is nothing to look up.
pub const INVALID_ALLOCATION_ID_MALFORMED: &str = "InvalidAllocationID.Malformed";

/// `Resource.AlreadyAssociated` (HTTP 400): the request asked for an association the
/// resource already has with something else, and did not allow that one to be replaced
/// ("you can specify false to ensure the operation fails if the Elastic IP address is
/// already associated with another resource" — `AssociateAddressRequest$AllowReassociation`).
pub const RESOURCE_ALREADY_ASSOCIATED: &str = "Resource.AlreadyAssociated";

/// `InvalidNetworkInterfaceID.NotFound` (HTTP 400): the request named a network interface
/// id this account holds no network interface for.
pub const INVALID_NETWORK_INTERFACE_ID_NOT_FOUND: &str = "InvalidNetworkInterfaceID.NotFound";

/// `InvalidNetworkInterfaceID.Malformed` (HTTP 400): the request named something that is
/// not a network interface id at all, so there is nothing to look up.
pub const INVALID_NETWORK_INTERFACE_ID_MALFORMED: &str = "InvalidNetworkInterfaceID.Malformed";

/// The id prefix of the association between an Elastic IP address and a resource, which
/// is what `ElasticIpAssociationId` names and `AssociateAddress` mints.
pub const ELASTIC_IP_ASSOCIATION_ID_PREFIX: &str = "eipassoc";

impl ComAmazonawsEc2Amazonec2State {
    /// The next unused Elastic IP address, or `None` when the search is exhausted.
    ///
    /// Drawn from `54.0.0.0/8`, a block Amazon advertises EC2 public addresses from, which
    /// is what "Amazon's pool of IPv4 addresses" (`AllocateAddress`) is here. The host
    /// part comes from the store's own monotonic counter and never from the request — a
    /// caller "cannot specify" which Amazon-pool address it gets — and the last octet skips
    /// `0` and `255`, which no interface is given. An address some allocation still holds
    /// is skipped, and the search probes at most [`MAX_ELASTIC_IP_PROBES`] candidates so it
    /// cannot spin.
    pub fn mint_elastic_ip(&mut self) -> Option<String> {
        for _ in 0..MAX_ELASTIC_IP_PROBES {
            self.next_elastic_ip = self.next_elastic_ip.wrapping_add(1);
            let host = self.next_elastic_ip;
            let candidate = format!(
                "54.{}.{}.{}",
                (host / (254 * 256)) % 256,
                (host / 254) % 256,
                1 + host % 254
            );
            if self.allocation_id_for_public_ip(&candidate).is_none() {
                return Some(candidate);
            }
        }
        None
    }

    /// The allocation id of the stored address whose `PublicIp` is `public_ip`, or `None`
    /// when the account holds no such address.
    ///
    /// A scan of the address store rather than a second map keyed by the address, so the
    /// public IP of an allocation is read off the one record that holds it; the store is
    /// bounded by [`MAX_STORED_ADDRESSES`], the way
    /// [`ComAmazonawsEc2Amazonec2State::launch_template_id_for_name`] scans the template
    /// store.
    pub fn allocation_id_for_public_ip(&self, public_ip: &str) -> Option<String> {
        self.addresses
            .iter()
            .find(|(_, address)| address.public_ip.as_deref() == Some(public_ip))
            .map(|(allocation_id, _)| allocation_id.clone())
    }

    /// The allocation id of the address currently associated with `instance_id`, or `None`
    /// when no address of the account is.
    ///
    /// Scanned off the address records themselves, like
    /// [`Self::allocation_id_for_public_ip`] and for the same reason: `Address$InstanceId`
    /// is where an association is stored, so there is no second copy of it to fall out of
    /// step. An instance holds at most one Elastic IP address — "if you associate an
    /// Elastic IP address with an instance that has an existing Elastic IP address, the
    /// existing address is disassociated from the instance" (`AssociateAddress`) — so the
    /// first match is the only one.
    pub fn allocation_id_associated_with_instance(&self, instance_id: &str) -> Option<String> {
        self.addresses
            .iter()
            .find(|(_, address)| address.instance_id.as_deref() == Some(instance_id))
            .map(|(allocation_id, _)| allocation_id.clone())
    }

    /// End `allocation_id`'s association, leaving the address ALLOCATED to the account:
    /// "the existing address is disassociated from the instance, but remains allocated to
    /// your account" (`AssociateAddress`).
    ///
    /// Shared because every path that ends an association does exactly this — the remap
    /// `AssociateAddress` performs, and `DisassociateAddress` and `ReleaseAddress` once
    /// they are implemented. The instance the address was associated with also gives up
    /// the public address it was reporting through it (`Instance$PublicIpAddress`), so the
    /// instance store and the address store cannot disagree about one address. Storing
    /// nothing new, it makes no byte charge.
    ///
    /// A no-op for an allocation id the account holds no address for, so a caller does not
    /// have to look one up first.
    pub fn detach_elastic_ip(&mut self, allocation_id: &str) {
        let Some(address) = self.addresses.get_mut(allocation_id) else {
            return;
        };
        let public_ip = address.public_ip.clone();
        let was_on = address.instance_id.take();
        address.association_id = None;
        address.network_interface_id = None;
        address.network_interface_owner_id = None;
        address.private_ip_address = None;
        let Some(instance) = was_on.and_then(|instance_id| self.instances.get_mut(&instance_id))
        else {
            return;
        };
        // Only the address this association carried: an instance reporting some other
        // public address keeps it.
        if instance.public_ip == public_ip {
            instance.public_ip = None;
        }
    }

    /// Store a newly allocated Elastic IP address, charging the caller-chosen bytes it
    /// holds against the store's byte budget so no path can store them uncharged.
    pub fn insert_address(&mut self, address: Address) -> String {
        let allocation_id = address.allocation_id.clone().unwrap_or_default();
        self.add_stored_bytes(address_payload_bytes(&address));
        self.addresses.insert(allocation_id.clone(), address);
        allocation_id
    }

    /// Whether another transit gateway fits inside the store's ceiling
    /// ([`MAX_TRANSIT_GATEWAYS`]).
    ///
    /// The same contract as [`Self::has_vpc_capacity`]: a create asks first and refuses
    /// with [`transit_gateway_limit_exceeded`] when it answers `false`.
    pub fn has_transit_gateway_capacity(&self) -> bool {
        self.transit_gateways.len() < MAX_TRANSIT_GATEWAYS
    }

    /// Insert a transit gateway in `pending`, with the default route table a create owes
    /// it, and answer the `TransitGatewayId` it was given.
    ///
    /// Everything the service itself owns is derived here rather than taken from the
    /// caller — the id, the ARN built from it, the owning account, the creation time, the
    /// state and the default route table's id — so two writers cannot report the same
    /// gateway differently. The option values in `spec.options` are the ones the request
    /// resolved ([`transit_gateway_options`]); the two route-table ids are filled in here
    /// because only this function knows the table it created.
    ///
    /// "When you create a transit gateway, we create a default transit gateway route table
    /// and use it as the default association route table and the default propagation route
    /// table … If you disable automatic route propagation, we do not create a default
    /// transit gateway route table" (`CreateTransitGateway`), and "Either
    /// `defaultRouteTableAssociation` or `defaultRouteTablePropagation` must be set to
    /// `enable` for Amazon Web Services Transit Gateway to create the default transit
    /// gateway route table" (`TransitGatewayOptions$DefaultRouteTableAssociation`) — so the
    /// table is created exactly when at least one of the two is enabled, and each id is
    /// reported only for the option that is.
    ///
    /// Like [`Self::insert_vpc`], the caller-chosen bytes of the record are charged against
    /// the store's byte budget here, so a writer cannot store them uncharged, and the
    /// clock-owed `available` move is recorded rather than slept for.
    pub fn insert_transit_gateway(&mut self, spec: TransitGatewaySpec) -> String {
        let transit_gateway_id = self.next_id("tgw");
        let creation_time = system_time_to_date_time(spec.created_at);
        let mut options = spec.options;
        let associates = options.default_route_table_association.as_deref() == Some(OPTION_ENABLE);
        let propagates = options.default_route_table_propagation.as_deref() == Some(OPTION_ENABLE);
        if associates || propagates {
            let route_table_id = self.next_id(TRANSIT_GATEWAY_ROUTE_TABLE_ID_PREFIX);
            self.transit_gateway_route_tables.insert(
                route_table_id.clone(),
                TransitGatewayRouteTable {
                    transit_gateway_route_table_id: Some(route_table_id.clone()),
                    transit_gateway_id: Some(transit_gateway_id.clone()),
                    // The table comes up with its gateway, so it starts `pending` too.
                    state: Some(TRANSIT_GATEWAY_ROUTE_TABLE_STATE_PENDING.to_string()),
                    default_association_route_table: Some(associates),
                    default_propagation_route_table: Some(propagates),
                    creation_time: Some(creation_time),
                    // Nothing tags the default route table: a `CreateTransitGateway` tag
                    // specification tags the gateway.
                    tags: Some(Vec::new()),
                },
            );
            if associates {
                options.association_default_route_table_id = Some(route_table_id.clone());
            }
            if propagates {
                options.propagation_default_route_table_id = Some(route_table_id);
            }
        }
        let transit_gateway = TransitGateway {
            transit_gateway_id: Some(transit_gateway_id.clone()),
            transit_gateway_arn: Some(::engine::emulator_arn(
                "ec2",
                &spec.region,
                &format!("transit-gateway/{transit_gateway_id}"),
            )),
            state: Some(TRANSIT_GATEWAY_STATE_PENDING.to_string()),
            owner_id: Some(::engine::emulator_account_id().to_string()),
            description: spec.description,
            creation_time: Some(creation_time),
            options: Some(options),
            tags: Some(spec.tags),
        };
        self.add_stored_bytes(transit_gateway_payload_bytes(&transit_gateway));
        self.transit_gateways
            .insert(transit_gateway_id.clone(), transit_gateway);
        // What the clock now owes this gateway: `available`, one TRANSITION_DELAY from
        // now — the same span, off the same monotonic clock, as a VPC's pending move.
        self.transit_gateway_available_at
            .insert(transit_gateway_id.clone(), Instant::now() + TRANSITION_DELAY);
        transit_gateway_id
    }
}

/// The caller-chosen bytes one stored Elastic IP address holds: its tags. Everything else
/// on the record — the allocation id, the address, the domain, the network border group —
/// is derived by this module or bounded by the model, so it is fixed-size.
///
/// The ONE definition of that charge: `AllocateAddress` asks it before the write and
/// charges exactly it, and a removal path must give back the same number.
pub fn address_payload_bytes(address: &Address) -> usize {
    tag_list_bytes(address.tags.as_deref().unwrap_or_default())
}

/// Whether `id` is a well-formed Elastic IP allocation id.
///
/// An allocation id is `eipalloc-` followed by 8 or 17 lowercase hexadecimal digits — the
/// short and the long id form, as the model's own id patterns read. EC2 answers
/// `InvalidAllocationID.Malformed` for anything else, BEFORE it looks the id up.
pub fn allocation_id_is_well_formed(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix("eipalloc-") else {
        return false;
    };
    matches!(suffix.len(), 8 | 17)
        && suffix
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Whether `candidate` is a dotted-quad IPv4 address, the form
/// `AllocateAddressRequest$Address` and `DescribeAddressesRequest$PublicIps` take.
///
/// `Ipv4Addr`'s own parser accepts exactly four decimal octets, so this is the one place
/// the form is decided: the crate's CIDR helpers are about blocks, not single addresses.
pub fn is_ipv4_address(candidate: &str) -> bool {
    candidate.parse::<::std::net::Ipv4Addr>().is_ok()
}

/// Refuse an allocation id the request named that this service cannot resolve: a
/// malformed one before an unknown one, the way the service orders the two.
pub fn require_known_address<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    allocation_id: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    if !allocation_id_is_well_formed(allocation_id) {
        return Err(invalid_allocation_id_malformed(allocation_id));
    }
    if !store.addresses.contains_key(allocation_id) {
        return Err(invalid_allocation_id_not_found(allocation_id));
    }
    Ok(())
}

/// Refuse a request that named something that is not an allocation id: HTTP 400
/// `InvalidAllocationID.Malformed`.
pub fn invalid_allocation_id_malformed<E>(allocation_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_ALLOCATION_ID_MALFORMED,
        400,
        format!("Invalid id: '{allocation_id}'"),
    )
}

/// Refuse a request that named an allocation id no address of the account has: HTTP 400
/// `InvalidAllocationID.NotFound`.
pub fn invalid_allocation_id_not_found<E>(allocation_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_ALLOCATION_ID_NOT_FOUND,
        400,
        format!("The allocation ID '{allocation_id}' does not exist"),
    )
}

/// Refuse a request that named an Elastic IP address this account holds no allocation
/// for: HTTP 400 `InvalidAddress.NotFound`.
pub fn invalid_address_not_found<E>(public_ip: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_ADDRESS_NOT_FOUND,
        400,
        format!("Address '{public_ip}' not found"),
    )
}

/// Whether `id` is a well-formed network interface id.
///
/// `eni-` followed by 8 or 17 lowercase hexadecimal digits — the short and the long EC2
/// resource id form, read the way [`allocation_id_is_well_formed`] reads an allocation id.
/// The all-zero suffix is WELL FORMED here, unlike an instance id's: AWS answers
/// `eni-00000000000000000` with `InvalidNetworkInterfaceID.NotFound` rather than
/// `.Malformed`, which is the counter-example [`instance_id_is_well_formed`] records.
pub fn network_interface_id_is_well_formed(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix("eni-") else {
        return false;
    };
    matches!(suffix.len(), 8 | 17)
        && suffix
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Refuse a request that named something that is not a network interface id: HTTP 400
/// `InvalidNetworkInterfaceID.Malformed`.
pub fn invalid_network_interface_id_malformed<E>(network_interface_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_NETWORK_INTERFACE_ID_MALFORMED,
        400,
        format!("Invalid id: '{network_interface_id}'"),
    )
}

/// Refuse a request that named a network interface id no interface of the account has:
/// HTTP 400 `InvalidNetworkInterfaceID.NotFound`.
pub fn invalid_network_interface_id_not_found<E>(network_interface_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_NETWORK_INTERFACE_ID_NOT_FOUND,
        400,
        format!("The networkInterface ID '{network_interface_id}' does not exist"),
    )
}

/// Refuse an association the resource already has with something else, which the request
/// did not allow to be replaced: HTTP 400 `Resource.AlreadyAssociated`.
pub fn resource_already_associated<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(RESOURCE_ALREADY_ASSOCIATED, 400, message)
}

/// Refuse an allocation that would take the store past [`MAX_STORED_ADDRESSES`]: HTTP 400
/// `AddressLimitExceeded`.
pub fn address_limit_exceeded<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(ADDRESS_LIMIT_EXCEEDED, 400, message)
}

/// The network border group a Region's own addresses are advertised from.
///
/// "A unique set of Availability Zones, Local Zones, or Wavelength Zones from which Amazon
/// Web Services advertises IP addresses" (`AllocateAddressRequest$NetworkBorderGroup`):
/// every Availability Zone of a Region shares one border group named for the Region, and a
/// Local or Wavelength Zone has its own. This emulator's zones are all
/// `availability-zone`s ([`availability_zones_in_region`]), so a Region has exactly this
/// one border group.
pub fn network_border_group_of_region(region: &str) -> String {
    region.to_string()
}

// ---------------------------------------------------------------------------
// Transit gateways
// ---------------------------------------------------------------------------

/// A transit gateway AWS is still creating: the `TransitGatewayState` enum's `pending`
/// member, which is what `CreateTransitGateway` answers with until [`TRANSITION_DELAY`]
/// has passed ("After the transit gateway enters the available state, you can attach your
/// VPCs and VPN connections to the transit gateway" — `CreateTransitGateway`).
pub const TRANSIT_GATEWAY_STATE_PENDING: &str = "pending";

/// A transit gateway that has finished coming up: the `TransitGatewayState` enum's
/// `available` member.
pub const TRANSIT_GATEWAY_STATE_AVAILABLE: &str = "available";

/// A transit gateway route table still coming up with its gateway: the
/// `TransitGatewayRouteTableState` enum's `pending` member.
pub const TRANSIT_GATEWAY_ROUTE_TABLE_STATE_PENDING: &str = "pending";

/// A transit gateway route table that is usable: the `TransitGatewayRouteTableState`
/// enum's `available` member.
pub const TRANSIT_GATEWAY_ROUTE_TABLE_STATE_AVAILABLE: &str = "available";

/// A transit gateway option turned ON: the `enable` member every one of the option enums
/// (`DnsSupportValue`, `VpnEcmpSupportValue`, …) is one of two values of.
pub const OPTION_ENABLE: &str = "enable";

/// A transit gateway option turned OFF: those enums' other member.
pub const OPTION_DISABLE: &str = "disable";

/// The `ResourceType` a `CreateTransitGateway` tag specification may name: the transit
/// gateway it creates.
pub const RESOURCE_TYPE_TRANSIT_GATEWAY: &str = "transit-gateway";

/// The ASN a transit gateway gets when the request names none: "The default is `64512`"
/// (`TransitGatewayRequestOptions$AmazonSideAsn`).
pub const DEFAULT_AMAZON_SIDE_ASN: i64 = 64_512;

/// The 16-bit private ASN range a transit gateway's Amazon-side ASN may come from: "The
/// range is 64512 to 65534 for 16-bit ASNs"
/// (`TransitGatewayRequestOptions$AmazonSideAsn`).
pub const AMAZON_SIDE_ASN_16_BIT: ::std::ops::RangeInclusive<i64> = 64_512..=65_534;

/// The 32-bit private ASN range: "and 4200000000 to 4294967294 for 32-bit ASNs"
/// (`TransitGatewayRequestOptions$AmazonSideAsn`).
pub const AMAZON_SIDE_ASN_32_BIT: ::std::ops::RangeInclusive<i64> = 4_200_000_000..=4_294_967_294;

/// The longest IPv4 prefix a transit gateway CIDR block may have: "Must be a size /24 CIDR
/// block or larger for IPv4" (`TransitGatewayRequestOptions$TransitGatewayCidrBlocks`) —
/// larger block, so a prefix no longer than this.
pub const MAX_TRANSIT_GATEWAY_IPV4_PREFIX: u8 = 24;

/// The longest IPv6 prefix a transit gateway CIDR block may have: "or a size /64 CIDR block
/// or larger for IPv6" (same member).
pub const MAX_TRANSIT_GATEWAY_IPV6_PREFIX: u8 = 64;

/// The largest number of transit gateways this in-memory store holds.
///
/// A capacity bound in the same style as [`MAX_VPCS`], not an AWS quota: each gateway is a
/// record in this process's memory, so the number a caller can create has to be finite.
/// [`ComAmazonawsEc2Amazonec2State::has_transit_gateway_capacity`] is the predicate a
/// writer asks before inserting; a create past it refuses with
/// [`TRANSIT_GATEWAY_LIMIT_EXCEEDED`].
pub const MAX_TRANSIT_GATEWAYS: usize = 1_000;

/// EC2's error code for a create that would take the account past its transit gateway
/// quota.
pub const TRANSIT_GATEWAY_LIMIT_EXCEEDED: &str = "TransitGatewayLimitExceeded";

/// EC2's error code for a request naming a transit gateway id this account holds no
/// transit gateway for.
pub const INVALID_TRANSIT_GATEWAY_ID_NOT_FOUND: &str = "InvalidTransitGatewayID.NotFound";

/// Everything [`ComAmazonawsEc2Amazonec2State::insert_transit_gateway`] needs that it does
/// not derive itself: what the REQUEST said, and the Region it was signed for.
#[derive(Debug, Clone)]
pub struct TransitGatewaySpec {
    /// "A description of the transit gateway" (`CreateTransitGatewayRequest$Description`),
    /// absent when the request gave none.
    pub description: Option<String>,
    /// The Region the request was signed for, which the gateway's ARN names.
    pub region: String,
    /// When the gateway was created, as the creation time every record of it reports.
    pub created_at: SystemTime,
    /// The resolved options ([`transit_gateway_options`]); the two default-route-table ids
    /// are filled in by the insert, which is what creates that table.
    pub options: TransitGatewayOptions,
    /// The tags of a `transit-gateway` tag specification.
    pub tags: Vec<Tag>,
}

/// The caller-chosen bytes one stored transit gateway holds: its tags, its description and
/// the CIDR blocks the request asked for. Everything else on the record — the ids, the
/// ARN, the account, the state, the option flags — is derived by this module or bounded by
/// the model, so it is fixed-size.
///
/// The ONE definition of that charge: `CreateTransitGateway` asks it before the write and
/// charges exactly it, and a removal path must give back the same number.
pub fn transit_gateway_payload_bytes(transit_gateway: &TransitGateway) -> usize {
    let tags = tag_list_bytes(transit_gateway.tags.as_deref().unwrap_or_default());
    let description = transit_gateway.description.as_deref().map_or(0, str::len);
    let cidr_blocks: usize = transit_gateway
        .options
        .as_ref()
        .and_then(|options| options.transit_gateway_cidr_blocks.as_deref())
        .unwrap_or_default()
        .iter()
        .map(|block| block.len())
        .sum();
    tags.saturating_add(description).saturating_add(cidr_blocks)
}

/// A requested transit gateway CIDR block in canonical form, of whichever family it names.
///
/// "One or more IPv4 or IPv6 CIDR blocks for the transit gateway. Must be a size /24 CIDR
/// block or larger for IPv4, or a size /64 CIDR block or larger for IPv6"
/// (`TransitGatewayRequestOptions$TransitGatewayCidrBlocks`): the family is decided by the
/// value itself, and each family's published minimum block size is the bound. The block is
/// read by the same two readers the VPC path uses, so this crate keeps ONE rule for what a
/// CIDR block is and only the published block size differs.
pub fn canonical_transit_gateway_cidr(value: &str) -> Result<String, CidrProblem> {
    if value.contains(':') {
        let canonical = canonical_ipv6_cidr(value)?;
        let (_, prefix_length) = canonical.split_once('/').ok_or(CidrProblem::Malformed)?;
        let prefix_length: u8 = prefix_length.parse().map_err(|_| CidrProblem::Malformed)?;
        if prefix_length > MAX_TRANSIT_GATEWAY_IPV6_PREFIX {
            return Err(CidrProblem::PrefixLength);
        }
        return Ok(canonical);
    }
    let canonical = canonical_ipv4_cidr(value)?;
    let (_, prefix_length) = split_ipv4_cidr(&canonical).ok_or(CidrProblem::Malformed)?;
    if prefix_length > MAX_TRANSIT_GATEWAY_IPV4_PREFIX {
        return Err(CidrProblem::PrefixLength);
    }
    Ok(canonical)
}

/// The options one transit gateway is created with: what the request asked for, each
/// member it left out at the default its own documentation publishes.
///
/// This is the ONE place those defaults live, so `CreateTransitGateway` and every later
/// reader of a gateway's options cannot disagree about a gateway nobody modified. The
/// defaults, from `TransitGatewayRequestOptions`'s own member docs:
///
/// * `AutoAcceptSharedAttachments` — "Disabled by default".
/// * `DefaultRouteTableAssociation` — "Enabled by default".
/// * `DefaultRouteTablePropagation` — "Enabled by default".
/// * `VpnEcmpSupport` — "Enabled by default".
/// * `DnsSupport` — "Enabled by default".
/// * `SecurityGroupReferencingSupport` — "This option is disabled by default".
/// * `MulticastSupport` — multicast is a gateway the caller asks for, so `disable`.
/// * `AmazonSideAsn` — [`DEFAULT_AMAZON_SIDE_ASN`].
///
/// A value outside the published range or an unknown enum member is REFUSED rather than
/// defaulted, so a typo never reads as "the default". The two default-route-table ids are
/// left absent: only [`ComAmazonawsEc2Amazonec2State::insert_transit_gateway`], which
/// creates that table, can name it.
pub fn transit_gateway_options<E>(
    requested: Option<&TransitGatewayRequestOptions>,
) -> Result<TransitGatewayOptions, Scaffolded<E>> {
    let asn = match requested.and_then(|options| options.amazon_side_asn) {
        None => DEFAULT_AMAZON_SIDE_ASN,
        Some(asn)
            if AMAZON_SIDE_ASN_16_BIT.contains(&asn) || AMAZON_SIDE_ASN_32_BIT.contains(&asn) =>
        {
            asn
        }
        Some(asn) => {
            return Err(invalid_parameter_value(format!(
                "Value ({asn}) for parameter amazonSideAsn is invalid. \
                 The range is {min_16} to {max_16} for 16-bit ASNs and \
                 {min_32} to {max_32} for 32-bit ASNs.",
                min_16 = AMAZON_SIDE_ASN_16_BIT.start(),
                max_16 = AMAZON_SIDE_ASN_16_BIT.end(),
                min_32 = AMAZON_SIDE_ASN_32_BIT.start(),
                max_32 = AMAZON_SIDE_ASN_32_BIT.end(),
            )))
        }
    };
    let mut cidr_blocks: Vec<String> = Vec::new();
    for requested_block in requested
        .and_then(|options| options.transit_gateway_cidr_blocks.as_deref())
        .unwrap_or_default()
    {
        match canonical_transit_gateway_cidr(requested_block) {
            Ok(canonical) => cidr_blocks.push(canonical),
            Err(CidrProblem::Malformed) => {
                return Err(invalid_parameter_value(format!(
                    "Value ({requested_block}) for parameter transitGatewayCidrBlocks is \
                     invalid. This is not a valid CIDR block."
                )))
            }
            Err(CidrProblem::PrefixLength) => {
                return Err(invalid_parameter_value(format!(
                    "The CIDR '{requested_block}' is invalid. Must be a size \
                     /{ipv4} CIDR block or larger for IPv4, or a size /{ipv6} CIDR block \
                     or larger for IPv6.",
                    ipv4 = MAX_TRANSIT_GATEWAY_IPV4_PREFIX,
                    ipv6 = MAX_TRANSIT_GATEWAY_IPV6_PREFIX,
                )))
            }
        }
    }
    Ok(TransitGatewayOptions {
        amazon_side_asn: Some(asn),
        // "One or more IPv4 or IPv6 CIDR blocks for the transit gateway": a gateway the
        // request asked for none for carries no member at all rather than an empty list.
        transit_gateway_cidr_blocks: (!cidr_blocks.is_empty()).then_some(cidr_blocks),
        auto_accept_shared_attachments: Some(transit_gateway_option(
            requested.and_then(|options| options.auto_accept_shared_attachments.as_deref()),
            "autoAcceptSharedAttachments",
            OPTION_DISABLE,
        )?),
        default_route_table_association: Some(transit_gateway_option(
            requested.and_then(|options| options.default_route_table_association.as_deref()),
            "defaultRouteTableAssociation",
            OPTION_ENABLE,
        )?),
        // Filled in by the insert, which is what creates the default route table.
        association_default_route_table_id: None,
        default_route_table_propagation: Some(transit_gateway_option(
            requested.and_then(|options| options.default_route_table_propagation.as_deref()),
            "defaultRouteTablePropagation",
            OPTION_ENABLE,
        )?),
        propagation_default_route_table_id: None,
        vpn_ecmp_support: Some(transit_gateway_option(
            requested.and_then(|options| options.vpn_ecmp_support.as_deref()),
            "vpnEcmpSupport",
            OPTION_ENABLE,
        )?),
        dns_support: Some(transit_gateway_option(
            requested.and_then(|options| options.dns_support.as_deref()),
            "dnsSupport",
            OPTION_ENABLE,
        )?),
        security_group_referencing_support: Some(transit_gateway_option(
            requested.and_then(|options| options.security_group_referencing_support.as_deref()),
            "securityGroupReferencingSupport",
            OPTION_DISABLE,
        )?),
        multicast_support: Some(transit_gateway_option(
            requested.and_then(|options| options.multicast_support.as_deref()),
            "multicastSupport",
            OPTION_DISABLE,
        )?),
        // "Defines if the Transit Gateway supports VPC Encryption Control"
        // (`TransitGatewayOptions$EncryptionSupport`): nothing in this request asks for
        // VPC Encryption Control, and no operation that would turn it on is implemented,
        // so no encryption state is reported rather than an invented one.
        encryption_support: None,
    })
}

/// One `enable`/`disable` transit gateway option: the requested value if the request
/// carried one, otherwise `default`.
///
/// Every one of these members targets an enum whose only members are [`OPTION_ENABLE`] and
/// [`OPTION_DISABLE`] (`smithy_query(query="shape",shape="DnsSupportValue")`), which the
/// boundary validator already refuses anything outside — so this is the backstop that
/// keeps a value no enum has out of the store whatever reaches the handler, and it never
/// lets a typo read as the default.
fn transit_gateway_option<E>(
    requested: Option<&str>,
    member: &str,
    default: &str,
) -> Result<String, Scaffolded<E>> {
    match requested {
        None => Ok(default.to_string()),
        Some(value) if value == OPTION_ENABLE || value == OPTION_DISABLE => Ok(value.to_string()),
        Some(value) => Err(invalid_parameter_value(format!(
            "Value ({value}) for parameter {member} is invalid. \
             Valid values are {OPTION_ENABLE} and {OPTION_DISABLE}."
        ))),
    }
}

/// Refuse a request that named a transit gateway id no transit gateway of the account has:
/// HTTP 400 `InvalidTransitGatewayID.NotFound`.
pub fn invalid_transit_gateway_id_not_found<E>(transit_gateway_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_TRANSIT_GATEWAY_ID_NOT_FOUND,
        400,
        format!("Transit Gateway {transit_gateway_id} was deleted or does not exist."),
    )
}

/// Refuse a create that would take the store past [`MAX_TRANSIT_GATEWAYS`]: HTTP 400
/// `TransitGatewayLimitExceeded`.
pub fn transit_gateway_limit_exceeded<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        TRANSIT_GATEWAY_LIMIT_EXCEEDED,
        400,
        format!(
            "The maximum number of transit gateways has been reached ({MAX_TRANSIT_GATEWAYS})"
        ),
    )
}

// ---------------------------------------------------------------------------
// Transit gateway route tables, and the route propagations into them
// ---------------------------------------------------------------------------

/// The id prefix every transit gateway route table is minted with, the one
/// [`ComAmazonawsEc2Amazonec2State::insert_transit_gateway`] gives the default route table
/// it creates.
///
/// A route table ANNOUNCEMENT id extends this prefix
/// ([`TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_ID_PREFIX`]), so the prefix alone cannot
/// separate the two kinds; it is the hex suffix [`prefixed_id_is_well_formed`] requires that
/// does, exactly as [`vpc_endpoint_id_is_well_formed`] separates the three `vpce-` kinds.
pub const TRANSIT_GATEWAY_ROUTE_TABLE_ID_PREFIX: &str = "tgw-rtb";

/// `InvalidRouteTableID.NotFound` (HTTP 400): the request named a route table id this
/// account holds no transit gateway route table for. EC2 answers the route table's own code
/// here rather than a transit-gateway-specific one.
pub const INVALID_ROUTE_TABLE_ID_NOT_FOUND: &str = "InvalidRouteTableID.NotFound";

/// `InvalidRouteTableId.Malformed` (HTTP 400): the request named something that is not a
/// transit gateway route table id at all, so there is nothing to look up.
pub const INVALID_ROUTE_TABLE_ID_MALFORMED: &str = "InvalidRouteTableId.Malformed";

/// Refuse a request that named a transit gateway route table id no route table of the
/// account has: HTTP 400 [`INVALID_ROUTE_TABLE_ID_NOT_FOUND`].
///
/// Worded like [`invalid_transit_gateway_id_not_found`], the refusal the gateway itself
/// answers, so one family of resources reads one way.
pub fn invalid_transit_gateway_route_table_id_not_found<E>(route_table_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_ROUTE_TABLE_ID_NOT_FOUND,
        400,
        format!("Transit Gateway Route Table {route_table_id} was deleted or does not exist."),
    )
}

/// A propagation whose enable is being carried out: the `TransitGatewayPropagationState`
/// enum's `enabling` member ([`crate::ops::types::TRANSIT_GATEWAY_PROPAGATION_STATE_VALUES`]).
pub const PROPAGATION_STATE_ENABLING: &str = "enabling";

/// A propagation in force: that enum's `enabled` member.
pub const PROPAGATION_STATE_ENABLED: &str = "enabled";

/// A propagation whose disable is being carried out: that enum's `disabling` member.
pub const PROPAGATION_STATE_DISABLING: &str = "disabling";

/// What the clock owes one stored route propagation next.
///
/// The `disabled` member of `TransitGatewayPropagationState` is deliberately not a move:
/// once the disable has been carried out the pair does not propagate at all, which is not a
/// propagation of that route table — the same reading as [`FastLaunchMove::Discontinued`],
/// where the state catalog has no word for "not configured" either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropagationMove {
    /// `enabling` -> `enabled`: the attachment's routes are in the table.
    Enabled,
    /// `disabling` -> reported no longer: the propagation is gone.
    Removed,
}

/// The move the clock owes one route propagation: a [`ClockTransition`] to the next
/// [`PropagationMove`].
pub type PropagationTransition = ClockTransition<PropagationMove>;

/// One attachment's propagation of routes into one transit gateway route table.
///
/// Flattened rather than holding a modelled shape, because the two shapes that report a
/// propagation carry different member sets — `TransitGatewayPropagation` names the route
/// table, `TransitGatewayRouteTablePropagation` does not, being reported under it — so
/// [`Self::view`] and [`Self::route_table_view`] build each from the one stored record and
/// the two reports cannot disagree.
#[derive(Debug, Clone)]
pub struct TransitGatewayPropagationRecord {
    /// The propagation route table the routes go into.
    pub transit_gateway_route_table_id: String,
    /// The attachment whose routes propagate.
    pub transit_gateway_attachment_id: String,
    /// The resource that attachment attaches, which a report of the propagation names as
    /// its `ResourceId`; absent for an attachment that carries no resource id.
    pub resource_id: Option<String>,
    /// That resource's `TransitGatewayAttachmentResourceType`.
    pub resource_type: String,
    /// The route table announcement the propagation was asked for through, when it was.
    pub transit_gateway_route_table_announcement_id: Option<String>,
    /// The propagation's state, a member of `TransitGatewayPropagationState`.
    pub state: String,
    /// The move the clock owes this propagation, while one is owed.
    pub pending: Option<PropagationTransition>,
}

impl TransitGatewayPropagationRecord {
    /// The propagation as `EnableTransitGatewayRouteTablePropagation` and
    /// `DisableTransitGatewayRouteTablePropagation` answer with it.
    pub fn view(&self) -> TransitGatewayPropagation {
        TransitGatewayPropagation {
            transit_gateway_attachment_id: Some(self.transit_gateway_attachment_id.clone()),
            resource_id: self.resource_id.clone(),
            resource_type: Some(self.resource_type.clone()),
            transit_gateway_route_table_id: Some(self.transit_gateway_route_table_id.clone()),
            state: Some(self.state.clone()),
            transit_gateway_route_table_announcement_id: self
                .transit_gateway_route_table_announcement_id
                .clone(),
        }
    }

    /// The propagation as `GetTransitGatewayRouteTablePropagations` reports it: the same
    /// record without the route table id, which that request named.
    pub fn route_table_view(&self) -> TransitGatewayRouteTablePropagation {
        TransitGatewayRouteTablePropagation {
            transit_gateway_attachment_id: Some(self.transit_gateway_attachment_id.clone()),
            resource_id: self.resource_id.clone(),
            resource_type: Some(self.resource_type.clone()),
            state: Some(self.state.clone()),
            transit_gateway_route_table_announcement_id: self
                .transit_gateway_route_table_announcement_id
                .clone(),
        }
    }

    /// Put the propagation into `transient` now and owe it `to` [`TRANSITION_DELAY`] later.
    ///
    /// The span is never zero, so a caller acting straight after the enabling or disabling
    /// call still sees the transient state and the refusals that go with it.
    pub fn begin_move(&mut self, transient: &str, to: PropagationMove) {
        self.state = transient.to_string();
        self.pending = Some(PropagationTransition::starting_now(to));
    }
}

/// What a propagation request asks of one (route table, attachment) pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropagationDecision {
    /// "Enables the specified attachment to propagate routes to the specified propagation
    /// route table" (`EnableTransitGatewayRouteTablePropagation`).
    Enable,
    /// "Disables the specified resource attachment from propagating routes to the specified
    /// propagation route table" (`DisableTransitGatewayRouteTablePropagation`).
    Disable,
}

/// The attachment, the route table announcement and the propagation route table one
/// propagation request names: the members
/// `Enable`/`DisableTransitGatewayRouteTablePropagationRequest` share.
#[derive(Debug, Clone, Copy)]
pub struct PropagationRequest<'a> {
    /// "The ID of the propagation route table" (`$TransitGatewayRouteTableId`), the one
    /// required member.
    pub route_table_id: &'a str,
    /// "The ID of the attachment" (`$TransitGatewayAttachmentId`), when the request names
    /// one.
    pub attachment_id: Option<&'a str>,
    /// "The ID of the route table announcement" (`$TransitGatewayRouteTableAnnouncementId`),
    /// when the request names one.
    pub announcement_id: Option<&'a str>,
}

/// Act on the route propagation `request` names, and answer it as it now stands.
///
/// The ONE place the rule the enabling and the disabling operation share is written, so
/// they cannot disagree about one pair:
///
/// * the propagation route table must be one the account holds, or there is nothing to
///   propagate into ([`invalid_transit_gateway_route_table_id_not_found`]);
/// * the attachment whose routes propagate is the one the request names, or — for a request
///   that names a route table announcement instead — the peering attachment that
///   announcement was made for, and a request that names neither has not said what to
///   propagate;
/// * that attachment must be one the account holds
///   ([`INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND`]);
/// * an enable needs a pair that does not propagate yet and a disable needs one that does,
///   so the other way round each is refused with `IncorrectState` and NOTHING is written;
/// * the pair then moves into the TRANSIENT state of the move asked for, with the clock
///   owing it the end of it ([`PropagationMove`]).
///
/// The CALLER refuses a malformed id first, before its own `DryRun` arm, so nothing here
/// depends on the request's form.
pub fn decide_transit_gateway_route_table_propagation<E>(
    store: &mut ComAmazonawsEc2Amazonec2State,
    request: PropagationRequest<'_>,
    decision: PropagationDecision,
) -> Result<TransitGatewayPropagation, Scaffolded<E>> {
    // The clock first: a propagation whose move has fallen due is already enabled, or gone,
    // so whether this call is admissible is asked of the state the pair is ACTUALLY in
    // rather than of the one it was left in.
    store.materialize_due_transitions();
    let route_table_id = request.route_table_id;
    if !store
        .transit_gateway_route_tables
        .contains_key(route_table_id)
    {
        return Err(invalid_transit_gateway_route_table_id_not_found(
            route_table_id,
        ));
    }

    // "The ID of the route table announcement": an announcement is a propagation of the
    // peering attachment it was made for, so the announcement names the attachment when the
    // request does not — and when the request names both they must be the same attachment,
    // rather than one silently winning over the other.
    let announced: Option<String> = match request.announcement_id {
        None => None,
        Some(announcement_id) => store
            .transit_gateway_route_table_announcements
            .get(announcement_id)
            .ok_or_else(|| {
                id_not_found(
                    INVALID_TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_ID_NOT_FOUND,
                    "transit gateway route table announcement ID",
                    announcement_id,
                )
            })?
            .view
            .peering_attachment_id
            .clone(),
    };
    let attachment_id = match (request.attachment_id, announced.as_deref()) {
        (Some(named), Some(announced)) if named != announced => {
            return Err(Scaffolded::invalid_parameter_combination(format!(
                "The attachment '{named}' is not the attachment the route table \
                 announcement was made for ('{announced}')"
            )))
        }
        (Some(named), _) => named.to_string(),
        (None, Some(announced)) => announced.to_string(),
        (None, None) => {
            return Err(Scaffolded::missing_parameter(
                "The request must name the attachment whose routes propagate, either as \
                 transitGatewayAttachmentId or through a route table announcement",
            ))
        }
    };

    let attachment = store
        .transit_gateway_attachments
        .get(&attachment_id)
        .ok_or_else(|| {
            id_not_found(
                INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND,
                "transit gateway attachment ID",
                &attachment_id,
            )
        })?;
    let resource_id = attachment.view.resource_id().map(str::to_string);
    let resource_type = attachment.view.resource_type();
    let key = (route_table_id.to_string(), attachment_id.clone());

    match decision {
        PropagationDecision::Enable => {
            // A pair that already propagates, or is still finishing a disable, has no
            // enable to perform: the request is refused and nothing is written, which is
            // how the first call's write is observable to a later one.
            if let Some(existing) = store.transit_gateway_propagations.get(&key) {
                return Err(incorrect_state(format!(
                    "The attachment '{attachment_id}' already propagates routes to the \
                     transit gateway route table '{route_table_id}' in state '{state}'",
                    state = existing.state,
                )));
            }
            if !store.has_transit_gateway_propagation_capacity() {
                return Err(resource_limit_exceeded(format!(
                    "This emulator holds {} route propagations, which is as many as it \
                     stores",
                    store.transit_gateway_propagations.len()
                )));
            }
            if store.would_exceed_stored_bytes(STORED_RECORD_OVERHEAD_BYTES) {
                return Err(resource_limit_exceeded(format!(
                    "This emulator holds {} of at most {MAX_STORED_BYTES} bytes of \
                     caller-supplied data, so it cannot store another route propagation",
                    store.stored_bytes()
                )));
            }
            let record = TransitGatewayPropagationRecord {
                transit_gateway_route_table_id: route_table_id.to_string(),
                transit_gateway_attachment_id: attachment_id,
                resource_id,
                resource_type: resource_type.to_string(),
                transit_gateway_route_table_announcement_id: request
                    .announcement_id
                    .map(str::to_string),
                // The TRANSIENT state of an enable, with the clock owing it `enabled`: the
                // span is never zero, so a caller acting straight after this call still
                // sees the pair enabling.
                state: PROPAGATION_STATE_ENABLING.to_string(),
                pending: Some(PropagationTransition::starting_now(
                    PropagationMove::Enabled,
                )),
            };
            let view = record.view();
            store.add_stored_bytes(STORED_RECORD_OVERHEAD_BYTES);
            store.transit_gateway_propagations.insert(key, record);
            Ok(view)
        }
        PropagationDecision::Disable => {
            let record = store
                .transit_gateway_propagations
                .get_mut(&key)
                .filter(|record| record.state != PROPAGATION_STATE_DISABLING)
                .ok_or_else(|| {
                    incorrect_state(format!(
                        "The attachment '{attachment_id}' does not propagate routes to the \
                         transit gateway route table '{route_table_id}'"
                    ))
                })?;
            record.begin_move(PROPAGATION_STATE_DISABLING, PropagationMove::Removed);
            Ok(record.view())
        }
    }
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another route propagation fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of
    /// [`Self::has_transit_gateway_attachment_capacity`].
    pub fn has_transit_gateway_propagation_capacity(&self) -> bool {
        self.transit_gateway_propagations.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// The route propagations of one transit gateway route table, in attachment-id order —
    /// the stable order `GetTransitGatewayRouteTablePropagations` reports and pages over.
    ///
    /// The store is keyed by (route table, attachment), so this is a RANGE over the keys of
    /// one route table rather than a scan of every propagation stored: the cost is
    /// proportional to the propagations of the table asked about.
    ///
    /// A read, so it writes nothing: the caller applies the clock-owed moves
    /// ([`Self::materialize_due_transitions`]) before asking, exactly as every describe of
    /// this module does.
    pub fn transit_gateway_route_table_propagations<'a>(
        &'a self,
        route_table_id: &'a str,
    ) -> impl Iterator<Item = &'a TransitGatewayPropagationRecord> {
        self.transit_gateway_propagations
            .range((route_table_id.to_string(), String::new())..)
            .take_while(move |((table, _), _)| table == route_table_id)
            .map(|(_, record)| record)
    }
}

// ---------------------------------------------------------------------------
// Dedicated Hosts
// ---------------------------------------------------------------------------

/// A Dedicated Host that is allocated and can accept launches: `AllocationState`'s
/// `available` (`smithy_query(query="shape",shape="AllocationState")`).
pub const HOST_STATE_AVAILABLE: &str = "available";

/// The auto-placement a Dedicated Host gets when the request names none: "Default:
/// `off`" (`AllocateHostsRequest$AutoPlacement`). A value of the modelled
/// `AutoPlacement` enum ([`crate::ops::types::AUTO_PLACEMENT_VALUES`]).
pub const DEFAULT_AUTO_PLACEMENT: &str = "off";

/// The host recovery a Dedicated Host gets when the request names none: "Host recovery is
/// disabled by default […] Default: `off`" (`AllocateHostsRequest$HostRecovery`). A value
/// of the modelled `HostRecovery` enum.
pub const DEFAULT_HOST_RECOVERY: &str = "off";

/// The AMD SEV-SNP setting a Dedicated Host gets when the request's `CpuOptions` names
/// none: "If you don't specify a value, AMD SEV-SNP is `disabled`"
/// (`HostCpuOptionsRequest$AmdSevSnp`). A value of the modelled `AmdSevSnp` enum
/// ([`crate::ops::types::AMD_SEV_SNP_VALUES`]).
pub const AMD_SEV_SNP_DISABLED: &str = "disabled";

/// A Dedicated Host that supports every instance type of ONE instance family:
/// "`on`[,] the Dedicated Host supports multiple instance types in the instance family"
/// (`Host$AllowsMultipleInstanceTypes`), which is the host an `InstanceFamily` request
/// allocates. A value of the modelled `AllowsMultipleInstanceTypes` enum
/// ([`crate::ops::types::ALLOWS_MULTIPLE_INSTANCE_TYPES_VALUES`]).
pub const ALLOWS_MULTIPLE_INSTANCE_TYPES_ON: &str = "on";

/// A Dedicated Host that supports ONE instance type only — "`off`[,] the Dedicated Host
/// supports a single instance type only" (`Host$AllowsMultipleInstanceTypes`), which is
/// the host an `InstanceType` request allocates.
pub const ALLOWS_MULTIPLE_INSTANCE_TYPES_OFF: &str = "off";

/// The one resource type `AllocateHosts`'s `TagSpecifications` can name: "The tags to
/// apply to the Dedicated Host during creation"
/// (`AllocateHostsRequest$TagSpecifications`), which is the `dedicated-host` member of the
/// modelled `ResourceType` enum ([`crate::ops::types::RESOURCE_TYPE_VALUES`]).
pub const DEDICATED_HOST_TAGGABLE_RESOURCE: &str = "dedicated-host";

/// How many Dedicated Hosts this emulator stores before `AllocateHosts` is refused.
///
/// CHOSEN, like [`MAX_STORED_VOLUMES`] and for the same reason: nothing in this emulator
/// reclaims a stored host, so the store needs a ceiling that is not the process's memory.
/// It is well above any published per-Region Dedicated Host quota a test would exercise.
/// A count ceiling is no bound on the caller-chosen bytes each record holds, so those are
/// charged against [`MAX_STORED_BYTES`] too ([`host_payload_bytes`]).
pub const MAX_STORED_DEDICATED_HOSTS: usize = 4096;

/// `HostLimitExceeded` (HTTP 400): the allocation would take the account past the number
/// of Dedicated Hosts it may hold.
pub const HOST_LIMIT_EXCEEDED: &str = "HostLimitExceeded";

/// `InvalidHostID.NotFound` (HTTP 400): the request named a host id the account holds no
/// such host for.
pub const INVALID_HOST_ID_NOT_FOUND: &str = "InvalidHostID.NotFound";

/// `InvalidHostID.Malformed` (HTTP 400): the request named something that is not a host id
/// at all, so there is nothing to look up.
pub const INVALID_HOST_ID_MALFORMED: &str = "InvalidHostID.Malformed";

/// The prefix EC2 names its Apple-hardware instance families with: `mac1`, `mac2`,
/// `mac2-m2pro`. The families themselves are read off the modelled `InstanceType` enum
/// rather than listed here — see [`is_mac_instance_family`].
pub const MAC_INSTANCE_FAMILY_PREFIX: &str = "mac";

/// The instance family part of an instance type: `mac2-m2pro` of `mac2-m2pro.metal`, `m5`
/// of `m5.large` — "The instance family […] For example, `m5`"
/// (`HostProperties$InstanceFamily`). A value carrying no size is its own family.
pub fn instance_family_of(instance_type: &str) -> &str {
    instance_type
        .split_once('.')
        .map_or(instance_type, |(family, _size)| family)
}

/// Whether the model declares `instance_type` at all: it is a value of the `InstanceType`
/// enum, the catalog [`offered_instance_types`] reports.
pub fn instance_type_is_modelled(instance_type: &str) -> bool {
    offered_instance_types().contains(&instance_type)
}

/// Whether `instance_family` is the family of at least one instance type the model
/// declares, so that a host allocated for it could ever run an instance.
pub fn instance_family_is_modelled(instance_family: &str) -> bool {
    offered_instance_types()
        .iter()
        .any(|instance_type| instance_family_of(instance_type) == instance_family)
}

/// Whether `instance_family` runs macOS, and so whether a Dedicated Host configured for it
/// is an EC2 Mac Dedicated Host.
///
/// EC2 names every Apple-hardware family with the [`MAC_INSTANCE_FAMILY_PREFIX`] and no
/// other family with it, and the families themselves come from the modelled `InstanceType`
/// enum — so this is the model's own answer rather than a list maintained here.
pub fn is_mac_instance_family(instance_family: &str) -> bool {
    instance_family.starts_with(MAC_INSTANCE_FAMILY_PREFIX)
        && instance_family_is_modelled(instance_family)
}

/// The macOS release lines each EC2 Mac instance family's hardware supports, newest first,
/// as `(instance family, releases)`.
///
/// Source: "Amazon EC2 Mac instances" in the Amazon EC2 User Guide, which pairs each Mac
/// instance type with the macOS releases its hardware runs — `mac1` is the x86 Mac mini
/// and reaches back to macOS Mojave (10.14), `mac2` the M1 Mac mini, which starts at Big
/// Sur (11), `mac2-m1ultra` the M1 Ultra Mac Studio, which starts at Monterey (12), and
/// the M2 families start at Ventura (13). Every Mac family of the modelled `InstanceType`
/// enum ([`is_mac_instance_family`]) appears exactly once.
///
/// Release LINES, not patch versions: which patch is current is Apple's to decide and
/// changes without any EC2 API call, and this emulator makes no outbound connection to
/// find out — so a line is the part of "the latest macOS versions that the EC2 Mac
/// Dedicated Host can launch without being upgraded" (`MacHost$MacOSLatestSupportedVersions`)
/// that is a stable fact of the host's hardware.
pub const MAC_OS_SUPPORTED_VERSIONS: [(&str, &[&str]); 5] = [
    ("mac1", &["15", "14", "13", "12", "11", "10.15", "10.14"]),
    ("mac2", &["15", "14", "13", "12", "11"]),
    ("mac2-m1ultra", &["15", "14", "13", "12"]),
    ("mac2-m2", &["15", "14", "13"]),
    ("mac2-m2pro", &["15", "14", "13"]),
];

/// The macOS release lines `instance_family`'s hardware supports, newest first, or `None`
/// for a family [`MAC_OS_SUPPORTED_VERSIONS`] publishes none for.
pub fn mac_os_supported_versions(instance_family: &str) -> Option<&'static [&'static str]> {
    MAC_OS_SUPPORTED_VERSIONS
        .iter()
        .find(|(family, _versions)| *family == instance_family)
        .map(|(_family, versions)| *versions)
}

/// The instance family a stored host is configured for: the family it was allocated for,
/// or the family of the one instance type it supports.
///
/// A host allocated for an `InstanceFamily` carries that family and no instance type, and
/// one allocated for an `InstanceType` carries both — "If the host supports multiple
/// instance types, no `instanceType` is returned" (`HostProperties$InstanceType`) — so the
/// family is the member every host has.
pub fn host_instance_family(host: &Host) -> Option<&str> {
    let properties = host.host_properties.as_ref()?;
    properties
        .instance_family
        .as_deref()
        .or_else(|| properties.instance_type.as_deref().map(instance_family_of))
}

/// `host` as the modelled [`MacHost`] shape, or `None` when it is not an EC2 Mac Dedicated
/// Host at all.
///
/// Both members are derived from the stored host rather than stored beside it: the id is
/// the host's own, and the macOS releases it can launch are a fact of its family's
/// hardware ([`mac_os_supported_versions`]), so nothing can report one host two ways.
pub fn mac_host_view(host: &Host) -> Option<MacHost> {
    let instance_family = host_instance_family(host)?;
    if !is_mac_instance_family(instance_family) {
        return None;
    }
    Some(MacHost {
        host_id: host.host_id.clone(),
        mac_os_latest_supported_versions: mac_os_supported_versions(instance_family).map(
            |versions| {
                versions
                    .iter()
                    .map(|version| (*version).to_string())
                    .collect()
            },
        ),
    })
}

/// The caller-chosen bytes one stored Dedicated Host holds: its tags and the `ClientToken`
/// the token map keeps for it. Everything else on the record — the id, the zone, the
/// family, the states, the account — is derived by this module or bounded by the model, so
/// it is fixed-size.
///
/// The ONE definition of that charge: `AllocateHosts` asks it before the write and charges
/// exactly it, and a removal path must give back the same number.
pub fn host_payload_bytes(host: &Host) -> usize {
    tag_list_bytes(host.tags.as_deref().unwrap_or_default())
        .saturating_add(host.client_token.as_deref().map_or(0, str::len))
}

/// Whether `id` is a well-formed Dedicated Host id.
///
/// A host id is `h-` followed by 8 or 17 lowercase hexadecimal digits — the short and the
/// long id form, as the model's own `^h-([0-9a-f]{8}|[0-9a-f]{17})$` style id patterns
/// read. EC2 answers `InvalidHostID.Malformed` for anything else, BEFORE it looks the id
/// up.
pub fn host_id_is_well_formed(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix("h-") else {
        return false;
    };
    matches!(suffix.len(), 8 | 17)
        && suffix
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Refuse a request that named a host id the account holds no such host for: HTTP 400
/// `InvalidHostID.NotFound`.
pub fn invalid_host_id_not_found<E>(host_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_HOST_ID_NOT_FOUND,
        400,
        format!("The host ID '{host_id}' does not exist"),
    )
}

/// Refuse a host id this service cannot resolve: a malformed one is answered as malformed
/// and a well-formed one as not found — the way the service orders the two, and the same
/// ordering [`require_known_volume`] keeps.
pub fn unresolvable_host_id<E>(host_id: &str) -> Scaffolded<E> {
    if host_id_is_well_formed(host_id) {
        return invalid_host_id_not_found(host_id);
    }
    Scaffolded::service_error(
        INVALID_HOST_ID_MALFORMED,
        400,
        format!("Invalid id: '{host_id}'"),
    )
}

/// Refuse an allocation that would take the store past [`MAX_STORED_DEDICATED_HOSTS`] or
/// its byte budget: HTTP 400 `HostLimitExceeded`, EC2's own Dedicated-Hosts quota code.
pub fn host_limit_exceeded<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(HOST_LIMIT_EXCEEDED, 400, message)
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another `count` Dedicated Hosts fit inside the store's ceiling
    /// ([`MAX_STORED_DEDICATED_HOSTS`]).
    ///
    /// An allocation must ask this BEFORE inserting anything and refuse with
    /// [`host_limit_exceeded`] when it answers `false`: one call allocates `Quantity`
    /// hosts, so the whole batch is gated rather than each host as it is written.
    pub fn has_dedicated_host_capacity(&self, count: usize) -> bool {
        self.dedicated_hosts.len().saturating_add(count) <= MAX_STORED_DEDICATED_HOSTS
    }

    /// Store one newly allocated Dedicated Host, charging the caller-chosen bytes it
    /// holds, and answer its id.
    pub fn insert_dedicated_host(&mut self, host: Host) -> String {
        let host_id = host.host_id.clone().unwrap_or_default();
        self.add_stored_bytes(host_payload_bytes(&host));
        self.dedicated_hosts.insert(host_id.clone(), host);
        host_id
    }
}

// =============================================================================
// VPC endpoint connections
// =============================================================================

/// The id prefix a stored VPC endpoint connection is minted with.
///
/// The model publishes no pattern for `VpcEndpointConnection$VpcEndpointConnectionId` (it
/// is a plain `String` — `smithy_query(query="shape",shape="VpcEndpointConnection")`), so
/// the id takes EC2's own `<prefix>-<hex>` form through [`ComAmazonawsEc2Amazonec2State::next_id`],
/// with a prefix naming the resource. It is minted by the store and never taken from a
/// request.
pub const VPC_ENDPOINT_CONNECTION_ID_PREFIX: &str = "vpce-conn";

/// The id prefix a stored VPC endpoint is minted with: `vpce-`, as
/// `DescribeVpcEndpoints`'s own evidence of an unknown endpoint id (`vpce-…`) spells it.
pub const VPC_ENDPOINT_ID_PREFIX: &str = "vpce";

/// The id prefix a stored VPC endpoint SERVICE configuration is minted with.
///
/// `vpce-svc-` rather than `vpce-`: a service id and an endpoint id are different
/// resources, and an endpoint id is therefore one whose prefix is `vpce-` and which is
/// NOT one of the longer `vpce-svc-`/`vpce-conn-` forms — see
/// [`vpc_endpoint_id_is_well_formed`].
pub const VPC_ENDPOINT_SERVICE_ID_PREFIX: &str = "vpce-svc";

/// Every state a VPC endpoint connection can be in, as the `vpc-endpoint-state` FILTER
/// spells it paired with the member of the model's `State` enum that names the same state.
///
/// Two sources spell one state two ways, and both are part of the contract: the filter
/// documents "`vpc-endpoint-state` - The state of the endpoint (`pendingAcceptance` |
/// `pending` | `available` | `deleting` | `deleted` | `rejected` | `failed`)"
/// (`DescribeVpcEndpointConnectionsRequest$Filters`), while `VpcEndpointConnection$VpcEndpointState`
/// targets the `State` enum, whose values are `PendingAcceptance`, `Pending`, `Available`,
/// `Deleting`, `Deleted`, `Rejected`, `Failed`, `Expired` and `Partial`
/// (`crate::ops::types::STATE_VALUES`). A stored connection therefore carries the MODELLED
/// value, and this table is what lets the documented filter value select it. `Expired` and
/// `Partial` are states of other resources the enum also serves — the filter documents no
/// value for either — so they are not here.
pub const VPC_ENDPOINT_STATE_FILTER_VALUES: [(&str, &str); 7] = [
    ("pendingAcceptance", "PendingAcceptance"),
    ("pending", "Pending"),
    ("available", "Available"),
    ("deleting", "Deleting"),
    ("deleted", "Deleted"),
    ("rejected", "Rejected"),
    ("failed", "Failed"),
];

/// A connection whose endpoint is waiting for the service owner to accept it: the `State`
/// enum's `PendingAcceptance` member, which is the state an endpoint to a service with
/// `AcceptanceRequired` enters.
pub const VPC_ENDPOINT_STATE_PENDING_ACCEPTANCE: &str = "PendingAcceptance";

/// A connection whose endpoint is in service: the `State` enum's `Available` member.
pub const VPC_ENDPOINT_STATE_AVAILABLE: &str = "Available";

/// A connection the service owner has accepted and that is not in service yet: the `State`
/// enum's `Pending` member, the non-terminal state an acceptance leaves it in.
pub const VPC_ENDPOINT_STATE_PENDING: &str = "Pending";

/// What one stored VPC endpoint connection is created with, beside the connection id the
/// store mints itself.
///
/// The spec of the resource, in the same shape [`SubnetSpec`] has for a subnet: an
/// operation that creates an endpoint to one of the account's services fills this in and
/// [`ComAmazonawsEc2Amazonec2State::insert_vpc_endpoint_connection`] turns it into the
/// modelled [`VpcEndpointConnection`] the describe reports, so the id, the stored key and
/// the byte charge are decided in exactly one place.
#[derive(Debug, Clone)]
pub struct VpcEndpointConnectionSpec {
    /// The service the endpoint is connected to (`VpcEndpointConnection$ServiceId`).
    pub service_id: String,
    /// The endpoint itself, which is also the key the connection is stored under.
    pub vpc_endpoint_id: String,
    /// The account that owns the endpoint (`VpcEndpointConnection$VpcEndpointOwner`).
    pub vpc_endpoint_owner: String,
    /// The endpoint's state, as the model's `State` enum spells it — see
    /// [`VPC_ENDPOINT_STATE_FILTER_VALUES`].
    pub vpc_endpoint_state: String,
    /// When the endpoint was created.
    pub creation_timestamp: SystemTime,
    /// "The Region of the endpoint" (`VpcEndpointConnection$VpcEndpointRegion`).
    pub vpc_endpoint_region: String,
    /// "The IP address type for the endpoint" (`VpcEndpointConnection$IpAddressType`), or
    /// `None` for an endpoint whose type carries none.
    pub ip_address_type: Option<String>,
    /// The DNS entries for the endpoint.
    pub dns_entries: Vec<DnsEntry>,
    /// The Network Load Balancers behind the service.
    pub network_load_balancer_arns: Vec<String>,
    /// The Gateway Load Balancers behind the service.
    pub gateway_load_balancer_arns: Vec<String>,
    /// The payer responsibility settings for the endpoint.
    pub payer_responsibilities: Vec<PayerResponsibilityEntry>,
    /// The tags on the endpoint.
    pub tags: Vec<Tag>,
}

/// How many VPC endpoint connections this emulator stores before a writer is refused.
///
/// CHOSEN, like [`MAX_STORED_DEDICATED_HOSTS`] and for the same reason: nothing in this
/// emulator reclaims a stored connection, and each one is keyed by a REQUEST-SUPPLIED
/// `VpcEndpointId`, so without a count ceiling repeated writes with fresh endpoint ids
/// would grow memory without bound. It is well above any published per-VPC VPC endpoint
/// quota a test would exercise. A count ceiling is no bound on the bytes each record
/// holds, so those are charged against [`MAX_STORED_BYTES`] too
/// ([`vpc_endpoint_connection_payload_bytes`]).
pub const MAX_STORED_VPC_ENDPOINT_CONNECTIONS: usize = 4096;

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another `count` VPC endpoint connections fit inside the store's ceiling
    /// ([`MAX_STORED_VPC_ENDPOINT_CONNECTIONS`]).
    ///
    /// A writer asks this BEFORE storing anything and refuses the request with EC2's own
    /// quota code when it answers `false`, the way [`Self::has_dedicated_host_capacity`]
    /// gates `AllocateHosts`. [`Self::insert_vpc_endpoint_connection`] asks it again for
    /// itself, so no path can get past the ceiling by forgetting to.
    pub fn has_vpc_endpoint_connection_capacity(&self, count: usize) -> bool {
        self.vpc_endpoint_connections
            .len()
            .saturating_add(count)
            <= MAX_STORED_VPC_ENDPOINT_CONNECTIONS
    }

    /// Store a connection to one of the account's own VPC endpoint services, and answer
    /// the `VpcEndpointConnectionId` it was given — or `None` when the store is full.
    ///
    /// The connection id is minted here rather than taken from `spec`, the way
    /// [`Self::insert_subnet`] mints a `SubnetId`, and the record is keyed by its
    /// `VpcEndpointId` because one endpoint connects to exactly one service. The bytes it
    /// holds are charged against the store's byte budget here, so no path can store them
    /// uncharged.
    ///
    /// `None` means the store is at [`MAX_STORED_VPC_ENDPOINT_CONNECTIONS`] and NOTHING
    /// was stored: the ceiling is enforced here as well as by
    /// [`Self::has_vpc_endpoint_connection_capacity`], so a writer that forgets the gate
    /// cannot grow the store without bound either. A writer that sees `None` refuses the
    /// request with EC2's own quota code rather than reporting a connection it does not
    /// hold. A connection REPLACING the one already stored under the same `VpcEndpointId`
    /// does not grow the store, so it is admitted whatever the count.
    pub fn insert_vpc_endpoint_connection(
        &mut self,
        spec: VpcEndpointConnectionSpec,
    ) -> Option<String> {
        // A write that REPLACES the connection of an endpoint already stored adds no
        // record, so only a new key has to fit inside the ceiling.
        let replaces_existing = self
            .vpc_endpoint_connections
            .contains_key(&spec.vpc_endpoint_id);
        if !replaces_existing && !self.has_vpc_endpoint_connection_capacity(1) {
            return None;
        }
        let vpc_endpoint_connection_id = self.next_id(VPC_ENDPOINT_CONNECTION_ID_PREFIX);
        let connection = VpcEndpointConnection {
            service_id: Some(spec.service_id),
            vpc_endpoint_id: Some(spec.vpc_endpoint_id),
            vpc_endpoint_owner: Some(spec.vpc_endpoint_owner),
            vpc_endpoint_state: Some(spec.vpc_endpoint_state),
            creation_timestamp: Some(system_time_to_date_time(spec.creation_timestamp)),
            dns_entries: Some(spec.dns_entries),
            network_load_balancer_arns: Some(spec.network_load_balancer_arns),
            gateway_load_balancer_arns: Some(spec.gateway_load_balancer_arns),
            ip_address_type: spec.ip_address_type,
            vpc_endpoint_connection_id: Some(vpc_endpoint_connection_id.clone()),
            tags: Some(spec.tags),
            vpc_endpoint_region: Some(spec.vpc_endpoint_region),
            payer_responsibilities: Some(spec.payer_responsibilities),
        };
        let vpc_endpoint_id = connection.vpc_endpoint_id.clone().unwrap_or_default();
        self.add_stored_bytes(vpc_endpoint_connection_payload_bytes(&connection));
        self.vpc_endpoint_connections
            .insert(vpc_endpoint_id, connection);
        Some(vpc_endpoint_connection_id)
    }
}

/// The bytes one stored VPC endpoint connection holds: the fixed overhead of the record
/// and the key it is stored under, plus the caller-chosen parts — its tags, the load
/// balancer ARNs of the service it connects to, and its DNS entries. The ids, the state,
/// the timestamp and the Region are derived by this module or bounded by the model, so they
/// are fixed-size, which is what the overhead constant stands for.
///
/// The overhead is charged so that a connection created with no tags, no ARNs and no DNS
/// entries is NOT free against the byte budget: every stored record costs something, so
/// repeated writes advance the budget whatever they carry.
///
/// The ONE definition of that charge, the way [`address_payload_bytes`] is for an address:
/// a writer charges exactly it, and a removal path must give back the same number.
pub fn vpc_endpoint_connection_payload_bytes(connection: &VpcEndpointConnection) -> usize {
    let arn_bytes = |arns: Option<&Vec<String>>| -> usize {
        arns.into_iter().flatten().map(String::len).sum()
    };
    let text_bytes = |value: Option<&String>| -> usize { value.map_or(0, String::len) };
    let dns_bytes: usize = connection
        .dns_entries
        .iter()
        .flatten()
        .map(|entry| {
            entry.dns_name.as_deref().unwrap_or_default().len()
                + entry.hosted_zone_id.as_deref().unwrap_or_default().len()
        })
        .sum();
    VPC_ENDPOINT_CONNECTION_RECORD_BYTES
        .saturating_add(
            connection
                .vpc_endpoint_id
                .as_deref()
                .unwrap_or_default()
                .len(),
        )
        .saturating_add(tag_list_bytes(
            connection.tags.as_deref().unwrap_or_default(),
        ))
        .saturating_add(arn_bytes(connection.network_load_balancer_arns.as_ref()))
        .saturating_add(arn_bytes(connection.gateway_load_balancer_arns.as_ref()))
        .saturating_add(dns_bytes)
        .saturating_add(text_bytes(connection.vpc_endpoint_owner.as_ref()))
        .saturating_add(text_bytes(connection.vpc_endpoint_region.as_ref()))
}

// =============================================================================
// A state move the clock owes, for the records that carry their own
// =============================================================================

/// A state a record is due to move to, and the instant the clock owes it.
///
/// The generic form of [`PendingTransition`], which is an instance's: a resource an
/// operation hands back in a NON-TERMINAL state ([`TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING`]
/// after an acceptance, say) still owes the move to its terminal one, and that move was due
/// at its instant whether or not anybody called again. The instant is RECORDED rather than
/// slept for, and [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] applies it
/// once the clock has passed it. A resource an operation hands back ready carries none.
#[derive(Debug, Clone)]
pub struct PendingStateMove {
    /// The state the record moves to once `due_at` has passed.
    pub to: String,
    /// When the move falls due, on the same monotonic clock and at the same
    /// [`TRANSITION_DELAY`] span as every other transition this store owes.
    pub due_at: Instant,
}

impl PendingStateMove {
    /// The move to `to`, falling due [`TRANSITION_DELAY`] from now.
    pub fn starting_now(to: impl Into<String>) -> Self {
        Self {
            to: to.into(),
            due_at: Instant::now() + TRANSITION_DELAY,
        }
    }

    /// The move, if it has come due by `now`.
    fn due_by(&self, now: Instant) -> Option<&str> {
        (now >= self.due_at).then_some(self.to.as_str())
    }
}

/// Whether `id` is a well-formed EC2 resource id of the kind `prefix` names.
///
/// An EC2 resource id is `<prefix>-<hex>`, with 8 or 17 lowercase hexadecimal digits — the
/// short and the long id form, as the model's own id patterns read
/// (`^vpc-([0-9a-f]{8}|[0-9a-f]{17})$`). The generic form of
/// [`volume_id_is_well_formed`], for the resources whose own `@pattern` the model does not
/// publish (`smithy_query(query="shape",shape="TransitGatewayAttachmentId")` is a plain
/// `String`): EC2 answers that resource's `Invalid<Kind>.Malformed` for anything else,
/// BEFORE it looks the id up, so a caller can tell "not an id of this kind" from "no such
/// resource".
pub fn prefixed_id_is_well_formed(prefix: &str, id: &str) -> bool {
    let Some(suffix) = id
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('-'))
    else {
        return false;
    };
    matches!(suffix.len(), 8 | 17)
        && suffix
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Whether `id` is a well-formed VPC ENDPOINT id, as opposed to an id of one of the two
/// other resources whose ids also begin `vpce-`.
///
/// [`VPC_ENDPOINT_SERVICE_ID_PREFIX`] and [`VPC_ENDPOINT_CONNECTION_ID_PREFIX`] extend the
/// endpoint's own prefix, so the prefix alone cannot separate them; it is the hex suffix
/// that does, an endpoint id carrying nothing but hex digits where those two carry `svc-`
/// or `conn-` first.
pub fn vpc_endpoint_id_is_well_formed(id: &str) -> bool {
    prefixed_id_is_well_formed(VPC_ENDPOINT_ID_PREFIX, id)
}

// =============================================================================
// Transit gateway attachments
// =============================================================================

/// The id prefix every transit gateway attachment is minted with, whatever resource it
/// attaches: one `TransitGatewayAttachmentId` space serves them all, which is why
/// [`ComAmazonawsEc2Amazonec2State::transit_gateway_attachments`] is one map.
pub const TRANSIT_GATEWAY_ATTACHMENT_ID_PREFIX: &str = "tgw-attach";

/// The id prefix a transit gateway multicast domain is minted with.
pub const TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_PREFIX: &str = "tgw-mcast-domain";

/// The id prefix a transit gateway route table announcement is minted with.
pub const TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_ID_PREFIX: &str = "tgw-rtb-announce";

/// An attachment waiting for the owner of the attached resource to accept it: the
/// `TransitGatewayAttachmentState` enum's `pendingAcceptance` member.
pub const TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE: &str = "pendingAcceptance";

/// An attachment whose acceptance is being carried out: the
/// `TransitGatewayAttachmentState` enum's `pending` member.
pub const TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING: &str = "pending";

/// An attachment in service: the `TransitGatewayAttachmentState` enum's `available` member.
pub const TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE: &str = "available";

/// An attachment whose rejection is being carried out: the
/// `TransitGatewayAttachmentState` enum's `rejecting` member.
pub const TRANSIT_GATEWAY_ATTACHMENT_STATE_REJECTING: &str = "rejecting";

/// An attachment the owner of the attached resource refused: the
/// `TransitGatewayAttachmentState` enum's `rejected` member.
pub const TRANSIT_GATEWAY_ATTACHMENT_STATE_REJECTED: &str = "rejected";

/// `pendingAcceptance` as the OTHER state enum spells it:
/// `TransitGatewayClientVpnAttachment$State` targets `TransitGatewayAttachmentStatusType`,
/// whose members are hyphenated (`pending-acceptance`, `pending`, `rejected`, `available`,
/// `deleting`, `deleted`) rather than camel-cased. One state, two spellings, both part of
/// the contract — see [`TransitGatewayAttachmentView::states`].
pub const CLIENT_VPN_ATTACHMENT_STATE_PENDING_ACCEPTANCE: &str = "pending-acceptance";

/// The `TransitGatewayAttachmentResourceType` of a VPC attachment: `vpc`.
pub const TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_VPC: &str = "vpc";

/// The `TransitGatewayAttachmentResourceType` of a transit gateway peering attachment:
/// `tgw-peering`.
pub const TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_PEERING: &str = "tgw-peering";

/// The `TransitGatewayAttachmentResourceType` of a Client VPN attachment: `client-vpn`.
pub const TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_CLIENT_VPN: &str = "client-vpn";

/// How one kind of attachment spells the states an acceptance or a rejection walks through.
///
/// The two state enums the model gives attachments name the same states with different
/// spellings, so the spellings are a property of the KIND rather than literals at a call
/// site: see [`TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE`] and
/// [`CLIENT_VPN_ATTACHMENT_STATE_PENDING_ACCEPTANCE`].
#[derive(Debug, Clone, Copy)]
pub struct TransitGatewayAttachmentStates {
    /// Waiting for the owner of the attached resource to accept.
    pub pending_acceptance: &'static str,
    /// The non-terminal state an accepted attachment passes through.
    pub accepting: &'static str,
    /// In service: what an accepted attachment reaches.
    pub available: &'static str,
    /// The non-terminal state a rejected attachment passes through.
    pub rejecting: &'static str,
    /// Refused: what a rejected attachment reaches.
    pub rejected: &'static str,
}

/// One transit gateway attachment, as the modelled shape its own describe reports.
///
/// Every kind shares the `TransitGatewayAttachmentId` space, so they share one store; each
/// kind keeps the shape the operations that report it answer with, so a member a writer
/// sets round-trips without a field-by-field copy.
#[derive(Debug, Clone)]
pub enum TransitGatewayAttachmentView {
    /// A VPC attachment (`AcceptTransitGatewayVpcAttachment`,
    /// `RejectTransitGatewayVpcAttachment`).
    Vpc(TransitGatewayVpcAttachment),
    /// A transit gateway peering attachment (`AcceptTransitGatewayPeeringAttachment`).
    Peering(TransitGatewayPeeringAttachment),
    /// A Client VPN attachment (`AcceptTransitGatewayClientVpnAttachment`).
    ClientVpn(TransitGatewayClientVpnAttachment),
}

impl TransitGatewayAttachmentView {
    /// How this kind spells the states of an acceptance and a rejection.
    ///
    /// `TransitGatewayAttachmentStatusType` — the Client VPN attachment's state enum — has
    /// no member for the transient state of a rejection, so for that kind `rejecting` is
    /// the terminal `rejected` itself.
    pub fn states(&self) -> TransitGatewayAttachmentStates {
        match self {
            Self::Vpc(_) | Self::Peering(_) => TransitGatewayAttachmentStates {
                pending_acceptance: TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING_ACCEPTANCE,
                accepting: TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING,
                available: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
                rejecting: TRANSIT_GATEWAY_ATTACHMENT_STATE_REJECTING,
                rejected: TRANSIT_GATEWAY_ATTACHMENT_STATE_REJECTED,
            },
            Self::ClientVpn(_) => TransitGatewayAttachmentStates {
                pending_acceptance: CLIENT_VPN_ATTACHMENT_STATE_PENDING_ACCEPTANCE,
                accepting: TRANSIT_GATEWAY_ATTACHMENT_STATE_PENDING,
                available: TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE,
                rejecting: TRANSIT_GATEWAY_ATTACHMENT_STATE_REJECTED,
                rejected: TRANSIT_GATEWAY_ATTACHMENT_STATE_REJECTED,
            },
        }
    }

    /// The attachment's `TransitGatewayAttachmentResourceType`.
    pub fn resource_type(&self) -> &'static str {
        match self {
            Self::Vpc(_) => TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_VPC,
            Self::Peering(_) => TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_PEERING,
            Self::ClientVpn(_) => TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_CLIENT_VPN,
        }
    }

    /// The id of the resource this attachment attaches, which is what a report of the
    /// attachment names as its `ResourceId`.
    pub fn resource_id(&self) -> Option<&str> {
        match self {
            Self::Vpc(attachment) => attachment.vpc_id.as_deref(),
            Self::Peering(attachment) => attachment
                .accepter_tgw_info
                .as_ref()
                .and_then(|info| info.transit_gateway_id.as_deref()),
            Self::ClientVpn(attachment) => attachment.client_vpn_endpoint_id.as_deref(),
        }
    }

    /// The account that owns the resource this attachment attaches.
    pub fn resource_owner_id(&self) -> Option<&str> {
        match self {
            Self::Vpc(attachment) => attachment.vpc_owner_id.as_deref(),
            Self::Peering(attachment) => attachment
                .accepter_tgw_info
                .as_ref()
                .and_then(|info| info.owner_id.as_deref()),
            Self::ClientVpn(attachment) => attachment.client_vpn_owner_id.as_deref(),
        }
    }

    /// The attachment's current state.
    pub fn state(&self) -> Option<&str> {
        match self {
            Self::Vpc(attachment) => attachment.state.as_deref(),
            Self::Peering(attachment) => attachment.state.as_deref(),
            Self::ClientVpn(attachment) => attachment.state.as_deref(),
        }
    }

    /// Move the attachment to `state`.
    ///
    /// A peering attachment also carries a `Status`, which is the status OF that state
    /// (`TransitGatewayPeeringAttachment$Status` — "the status of the transit gateway
    /// peering attachment"), so it is moved with it rather than left naming the previous
    /// one.
    pub fn set_state(&mut self, state: &str) {
        match self {
            Self::Vpc(attachment) => attachment.state = Some(state.to_string()),
            Self::Peering(attachment) => {
                attachment.state = Some(state.to_string());
                attachment.status = Some(PeeringAttachmentStatus {
                    code: Some(state.to_string()),
                    message: None,
                });
            }
            Self::ClientVpn(attachment) => attachment.state = Some(state.to_string()),
        }
    }

    /// Stamp `attachment_id` onto the attachment, which the store mints rather than taking
    /// from a request.
    fn set_attachment_id(&mut self, attachment_id: &str) {
        match self {
            Self::Vpc(attachment) => {
                attachment.transit_gateway_attachment_id = Some(attachment_id.to_string());
            }
            Self::Peering(attachment) => {
                attachment.transit_gateway_attachment_id = Some(attachment_id.to_string());
            }
            Self::ClientVpn(attachment) => {
                attachment.transit_gateway_attachment_id = Some(attachment_id.to_string());
            }
        }
    }

    /// The caller-chosen bytes this attachment holds: its tags, the only member of any kind
    /// that a caller sizes.
    fn payload_bytes(&self) -> usize {
        match self {
            Self::Vpc(attachment) => tag_list_bytes(attachment.tags.as_deref().unwrap_or_default()),
            Self::Peering(attachment) => {
                tag_list_bytes(attachment.tags.as_deref().unwrap_or_default())
            }
            Self::ClientVpn(_) => 0,
        }
    }
}

/// One stored transit gateway attachment: the modelled view its describe reports, plus the
/// state move the clock still owes it.
#[derive(Debug, Clone)]
pub struct TransitGatewayAttachmentRecord {
    /// The attachment itself.
    pub view: TransitGatewayAttachmentView,
    /// The move to the terminal state of an acceptance or a rejection, for an attachment
    /// one of those left in a transient state; `None` for an attachment that owes none.
    pub pending: Option<PendingStateMove>,
}

impl TransitGatewayAttachmentRecord {
    /// Put the attachment into `transient` now and owe it `terminal`
    /// [`TRANSITION_DELAY`] later.
    ///
    /// The span is never zero, so a caller acting straight after the accepting or rejecting
    /// call still sees the transient state and the state errors that go with it.
    pub fn begin_move(&mut self, transient: &str, terminal: &str) {
        self.view.set_state(transient);
        self.pending = Some(PendingStateMove::starting_now(terminal));
    }
}

/// One subnet's association with a transit gateway multicast domain.
///
/// Flattened rather than holding the modelled
/// [`TransitGatewayMulticastDomainAssociation`], which carries exactly one subnet: the
/// association IS per subnet, so the record is per subnet and [`Self::view`] builds the
/// modelled shape when one is reported.
#[derive(Debug, Clone)]
pub struct MulticastDomainAssociationRecord {
    /// The attachment whose resource is associated with the domain.
    pub transit_gateway_attachment_id: String,
    /// The resource the attachment attaches.
    pub resource_id: String,
    /// That resource's `TransitGatewayAttachmentResourceType`.
    pub resource_type: String,
    /// The account that owns the resource.
    pub resource_owner_id: String,
    /// The associated subnet.
    pub subnet_id: String,
    /// The association's state, a member of
    /// `TransitGatewayMulitcastDomainAssociationState`.
    pub state: String,
    /// The move to `associated` an acceptance owes this association.
    pub pending: Option<PendingStateMove>,
}

impl MulticastDomainAssociationRecord {
    /// The modelled association a report of this record answers with.
    pub fn view(&self) -> TransitGatewayMulticastDomainAssociation {
        TransitGatewayMulticastDomainAssociation {
            transit_gateway_attachment_id: Some(self.transit_gateway_attachment_id.clone()),
            resource_id: Some(self.resource_id.clone()),
            resource_type: Some(self.resource_type.clone()),
            resource_owner_id: Some(self.resource_owner_id.clone()),
            subnet: Some(self.subnet_view()),
        }
    }

    /// The modelled subnet association this record is.
    pub fn subnet_view(&self) -> SubnetAssociation {
        SubnetAssociation {
            subnet_id: Some(self.subnet_id.clone()),
            state: Some(self.state.clone()),
        }
    }
}

/// An association waiting for the domain owner to accept it: the
/// `TransitGatewayMulitcastDomainAssociationState` enum's `pendingAcceptance` member.
pub const MULTICAST_ASSOCIATION_STATE_PENDING_ACCEPTANCE: &str = "pendingAcceptance";

/// An association being carried out: that enum's `associating` member.
pub const MULTICAST_ASSOCIATION_STATE_ASSOCIATING: &str = "associating";

/// An association in force: that enum's `associated` member.
pub const MULTICAST_ASSOCIATION_STATE_ASSOCIATED: &str = "associated";

/// One transit gateway multicast domain and the resources associated with it.
#[derive(Debug, Clone)]
pub struct TransitGatewayMulticastDomainRecord {
    /// The domain's own id, minted by the store.
    pub transit_gateway_multicast_domain_id: String,
    /// The transit gateway the domain belongs to.
    pub transit_gateway_id: String,
    /// The account that owns the domain.
    pub owner_id: String,
    /// The domain's state, a member of `TransitGatewayMulticastDomainState`.
    pub state: String,
    /// The subnet associations of the domain, one per (attachment, subnet).
    pub associations: Vec<MulticastDomainAssociationRecord>,
}

/// One stored transit gateway route table announcement.
#[derive(Debug, Clone)]
pub struct TransitGatewayRouteTableAnnouncementRecord {
    /// The announcement itself, as the modelled shape its operations answer with.
    pub view: TransitGatewayRouteTableAnnouncement,
    /// The move to `deleted` a delete owes this announcement.
    pub pending: Option<PendingStateMove>,
}

/// An announcement in force: the `TransitGatewayRouteTableAnnouncementState` enum's
/// `available` member.
pub const ANNOUNCEMENT_STATE_AVAILABLE: &str = "available";

/// An announcement a delete has started on: that enum's `deleting` member.
pub const ANNOUNCEMENT_STATE_DELETING: &str = "deleting";

/// An announcement a delete has finished with: that enum's `deleted` member.
pub const ANNOUNCEMENT_STATE_DELETED: &str = "deleted";

// =============================================================================
// IPAM pools, and the one BYOIP state the release path has no word for
// =============================================================================

/// The id prefix an IPAM pool is minted with.
pub const IPAM_POOL_ID_PREFIX: &str = "ipam-pool";

/// A range whose advertisement has been asked for and is not in force yet: the
/// `ByoipCidrState` enum's `pending-advertising` member.
///
/// The rest of that catalog, the store of ranges ([`ByoipCidrRecord`]) and the CIDR
/// canonicalisation ([`canonical_byoip_cidr`]) are the BYOIP section above — this is the one
/// member the provision/release path has no use for, and an advertisement is the only
/// operation that puts a range into it.
pub const BYOIP_CIDR_STATE_PENDING_ADVERTISING: &str = "pending-advertising";

/// The `IpamScopeType` of a pool in the public scope, which is the only scope a BYOIP CIDR
/// can be moved into: "Only used for provisioning IP address CIDRs to pools in the public
/// scope" (`IpamPool$PublicIpSource`).
pub const IPAM_SCOPE_TYPE_PUBLIC: &str = "public";

/// The `AddressFamily` of an IPv4 pool or range.
pub const ADDRESS_FAMILY_IPV4: &str = "ipv4";

/// The `AddressFamily` of an IPv6 pool or range.
pub const ADDRESS_FAMILY_IPV6: &str = "ipv6";

/// The address family a canonical BYOIP range belongs to, as `AddressFamily` spells it.
pub fn byoip_cidr_address_family(cidr: &str) -> &'static str {
    if cidr.contains(':') {
        ADDRESS_FAMILY_IPV6
    } else {
        ADDRESS_FAMILY_IPV4
    }
}

// =============================================================================
// Error codes of the resources above
// =============================================================================

/// `InvalidTransitGatewayAttachmentID.NotFound` (HTTP 400): the request named a transit
/// gateway attachment id this account holds no attachment for.
pub const INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND: &str =
    "InvalidTransitGatewayAttachmentID.NotFound";

/// `InvalidTransitGatewayAttachmentID.Malformed` (HTTP 400): the request named something
/// that is not a transit gateway attachment id at all.
pub const INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MALFORMED: &str =
    "InvalidTransitGatewayAttachmentID.Malformed";

/// `InvalidTransitGatewayMulticastDomainId.NotFound` (HTTP 400).
pub const INVALID_TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_NOT_FOUND: &str =
    "InvalidTransitGatewayMulticastDomainId.NotFound";

/// `InvalidTransitGatewayMulticastDomainId.Malformed` (HTTP 400).
pub const INVALID_TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_MALFORMED: &str =
    "InvalidTransitGatewayMulticastDomainId.Malformed";

/// `InvalidTransitGatewayRouteTableAnnouncementId.NotFound` (HTTP 400).
pub const INVALID_TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_ID_NOT_FOUND: &str =
    "InvalidTransitGatewayRouteTableAnnouncementId.NotFound";

/// `InvalidTransitGatewayRouteTableAnnouncementId.Malformed` (HTTP 400).
pub const INVALID_TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_ID_MALFORMED: &str =
    "InvalidTransitGatewayRouteTableAnnouncementId.Malformed";

/// `InvalidVpcEndpointId.NotFound` (HTTP 400): the request named a VPC endpoint id this
/// account holds no endpoint for.
pub const INVALID_VPC_ENDPOINT_ID_NOT_FOUND: &str = "InvalidVpcEndpointId.NotFound";

/// `InvalidVpcEndpointId.Malformed` (HTTP 400).
pub const INVALID_VPC_ENDPOINT_ID_MALFORMED: &str = "InvalidVpcEndpointId.Malformed";

/// `InvalidVpcEndpointServiceId.NotFound` (HTTP 400): the request named a VPC endpoint
/// service id this account holds no endpoint service for.
pub const INVALID_VPC_ENDPOINT_SERVICE_ID_NOT_FOUND: &str =
    "InvalidVpcEndpointServiceId.NotFound";

/// `InvalidVpcEndpointServiceId.Malformed` (HTTP 400).
pub const INVALID_VPC_ENDPOINT_SERVICE_ID_MALFORMED: &str =
    "InvalidVpcEndpointServiceId.Malformed";

/// `InvalidIpamPoolId.NotFound` (HTTP 400): the request named an IPAM pool id this account
/// holds no pool for.
pub const INVALID_IPAM_POOL_ID_NOT_FOUND: &str = "InvalidIpamPoolId.NotFound";

/// `InvalidIpamPoolId.Malformed` (HTTP 400).
pub const INVALID_IPAM_POOL_ID_MALFORMED: &str = "InvalidIpamPoolId.Malformed";

/// Refuse an id of the kind `prefix` names that this service cannot resolve: a malformed
/// one with `malformed`, an unknown one with `not_found`, in that order — the order the
/// service asks the two questions in, so "not an id of this kind" is never reported as "no
/// such resource".
///
/// The ONE place that ordering lives, so every resource of this module answers it the same
/// way; [`require_known_volume`] is the same contract written out for volumes.
pub fn require_well_formed_id<E>(
    prefix: &str,
    id: &str,
    malformed: &'static str,
) -> Result<(), Scaffolded<E>> {
    if prefixed_id_is_well_formed(prefix, id) {
        return Ok(());
    }
    Err(Scaffolded::service_error(
        malformed,
        400,
        format!("Invalid id: '{id}'"),
    ))
}

/// Refuse a request that named a resource id this account holds no resource for: HTTP 400
/// with `code`, which is that resource's own `Invalid<Kind>.NotFound`.
pub fn id_not_found<E>(code: &'static str, kind: &str, id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(code, 400, format!("The {kind} '{id}' does not exist"))
}

/// What the owner of an attached resource decided about a transit gateway attachment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentDecision {
    /// Accept it: "Accepts a request to attach a VPC to a transit gateway"
    /// (`AcceptTransitGatewayVpcAttachment`).
    Accept,
    /// Reject it: "Rejects a request to attach a VPC to a transit gateway"
    /// (`RejectTransitGatewayVpcAttachment`).
    Reject,
}

impl AttachmentDecision {
    /// The decision as the refusal below names it.
    fn verb(self) -> &'static str {
        match self {
            Self::Accept => "accepted",
            Self::Reject => "rejected",
        }
    }
}

/// Act on the transit gateway attachment `attachment_id` names, and answer it as it now
/// stands.
///
/// The ONE place the rule every accepting and rejecting operation of the attachment family
/// follows is written: "the attachment must be in the `pendingAcceptance` state"
/// (`AcceptTransitGatewayVpcAttachment`), so an attachment in any other state is refused
/// with `IncorrectState` and nothing is written; an accepted one moves to the transient
/// state of an acceptance with the clock owing it `available`, a rejected one to the
/// transient state of a rejection with `rejected` owed.
///
/// `resource_type` is the
/// `TransitGatewayAttachmentResourceType` the calling operation acts on — every kind shares
/// one id space, so an id naming an attachment of ANOTHER kind names no attachment this
/// operation can act on and is refused as unknown. The CALLER refuses a malformed
/// `attachment_id` first, before its own `DryRun` arm, so that nothing here depends on the
/// request's form.
pub fn decide_transit_gateway_attachment<E>(
    store: &mut ComAmazonawsEc2Amazonec2State,
    attachment_id: &str,
    resource_type: &'static str,
    decision: AttachmentDecision,
) -> Result<TransitGatewayAttachmentView, Scaffolded<E>> {
    // The clock first: an attachment whose move has fallen due is already in its terminal
    // state, so whether this call is admissible is asked of the state the attachment is
    // ACTUALLY in rather than of the one it was left in.
    store.materialize_due_transitions();
    let record = store
        .transit_gateway_attachments
        .get_mut(attachment_id)
        .filter(|record| record.view.resource_type() == resource_type)
        .ok_or_else(|| {
            id_not_found(
                INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND,
                "transit gateway attachment ID",
                attachment_id,
            )
        })?;
    let states = record.view.states();
    if record.view.state() != Some(states.pending_acceptance) {
        return Err(incorrect_state(format!(
            "The transit gateway attachment '{attachment_id}' is in state '{state}'; only an \
             attachment in state '{pending_acceptance}' can be {verb}",
            state = record.view.state().unwrap_or_default(),
            pending_acceptance = states.pending_acceptance,
            verb = decision.verb(),
        )));
    }
    match decision {
        AttachmentDecision::Accept => record.begin_move(states.accepting, states.available),
        AttachmentDecision::Reject => record.begin_move(states.rejecting, states.rejected),
    }
    Ok(record.view.clone())
}

/// The server error a handler answers when the attachment this module just resolved for it
/// turns out to be of another kind — which the resolution above has already ruled out, so
/// reaching it means this module is inconsistent with itself rather than that the request
/// was wrong.
pub fn unexpected_attachment_kind<E>(attachment_id: &str) -> Scaffolded<E> {
    internal_error(format!(
        "the attachment '{attachment_id}' resolved a moment ago is not of the kind this \
         operation acts on"
    ))
}

// =============================================================================
// Writers for the resources above
// =============================================================================

/// How many records each of the stores the writers below fill holds before a writer is
/// refused.
///
/// CHOSEN, like [`MAX_STORED_VPC_ENDPOINT_CONNECTIONS`] and [`MAX_STORED_DEDICATED_HOSTS`]
/// and for the same reason: nothing in this emulator reclaims one of these records, and
/// every write mints a FRESH id, so without a count ceiling repeated writes would grow
/// memory without bound. ONE number for all of them because they are bounded for the same
/// reason and at the same scale, and it is well above any published quota a test would
/// exercise. A count ceiling is no bound on the BYTES each record holds, so every write
/// also charges [`STORED_RECORD_OVERHEAD_BYTES`] on top of the caller-chosen text the
/// record carries.
pub const MAX_STORED_RECORDS_PER_STORE: usize = 4096;

/// The fixed overhead one of the records the writers below store costs whatever it
/// carries: the modelled record itself and the map entry holding it, as a CHOSEN constant
/// in the same spirit as [`VPC_ENDPOINT_CONNECTION_RECORD_BYTES`] — the point is that a
/// record created with no tags and no optional text is NOT free against
/// [`MAX_STORED_BYTES`], so repeated writes advance the budget whatever they carry, not
/// that it is byte-exact.
pub const STORED_RECORD_OVERHEAD_BYTES: usize = 512;

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another transit gateway attachment fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]).
    ///
    /// The same contract as [`Self::has_vpc_endpoint_connection_capacity`]: a writer asks
    /// this BEFORE storing anything and refuses the request with EC2's own quota code when
    /// it answers `false`, and [`Self::insert_transit_gateway_attachment`] asks it again
    /// for itself, so no path can get past the ceiling by forgetting to.
    pub fn has_transit_gateway_attachment_capacity(&self) -> bool {
        self.transit_gateway_attachments.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Whether another transit gateway multicast domain fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of
    /// [`Self::has_transit_gateway_attachment_capacity`].
    pub fn has_transit_gateway_multicast_domain_capacity(&self) -> bool {
        self.transit_gateway_multicast_domains.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Whether another transit gateway route table announcement fits inside the store's
    /// ceiling ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of
    /// [`Self::has_transit_gateway_attachment_capacity`].
    pub fn has_transit_gateway_route_table_announcement_capacity(&self) -> bool {
        self.transit_gateway_route_table_announcements.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Whether another VPC endpoint service configuration fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of
    /// [`Self::has_transit_gateway_attachment_capacity`].
    pub fn has_vpc_endpoint_service_capacity(&self) -> bool {
        self.vpc_endpoint_services.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Whether another VPC endpoint fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of
    /// [`Self::has_transit_gateway_attachment_capacity`].
    pub fn has_vpc_endpoint_capacity(&self) -> bool {
        self.vpc_endpoints.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Whether another IPAM pool fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of
    /// [`Self::has_transit_gateway_attachment_capacity`].
    pub fn has_ipam_pool_capacity(&self) -> bool {
        self.ipam_pools.len() < MAX_STORED_RECORDS_PER_STORE
    }
}

impl ComAmazonawsEc2Amazonec2State {
    /// Store a transit gateway attachment and answer the `TransitGatewayAttachmentId` it
    /// was given — or `None` when the store is full.
    ///
    /// The id is minted here rather than taken from `view`, the way
    /// [`Self::insert_vpc_endpoint_connection`] mints a connection id, and the
    /// caller-chosen bytes the record holds are charged here on top of the fixed
    /// [`STORED_RECORD_OVERHEAD_BYTES`] every stored record costs, so no path can store a
    /// record uncharged and a tag-free attachment is not free against the byte budget.
    ///
    /// `None` means the store is at [`MAX_STORED_RECORDS_PER_STORE`] and NOTHING was
    /// stored; a writer that sees it refuses the request with EC2's own quota code rather
    /// than reporting an attachment it does not hold.
    pub fn insert_transit_gateway_attachment(
        &mut self,
        mut view: TransitGatewayAttachmentView,
    ) -> Option<String> {
        if !self.has_transit_gateway_attachment_capacity() {
            return None;
        }
        let attachment_id = self.next_id(TRANSIT_GATEWAY_ATTACHMENT_ID_PREFIX);
        view.set_attachment_id(&attachment_id);
        self.add_stored_bytes(STORED_RECORD_OVERHEAD_BYTES + view.payload_bytes());
        self.transit_gateway_attachments.insert(
            attachment_id.clone(),
            TransitGatewayAttachmentRecord {
                view,
                pending: None,
            },
        );
        Some(attachment_id)
    }

    /// Store a transit gateway multicast domain and answer the
    /// `TransitGatewayMulticastDomainId` it was given — or `None` when the store is at
    /// [`MAX_STORED_RECORDS_PER_STORE`] and nothing was stored.
    pub fn insert_transit_gateway_multicast_domain(
        &mut self,
        mut record: TransitGatewayMulticastDomainRecord,
    ) -> Option<String> {
        if !self.has_transit_gateway_multicast_domain_capacity() {
            return None;
        }
        let domain_id = self.next_id(TRANSIT_GATEWAY_MULTICAST_DOMAIN_ID_PREFIX);
        record.transit_gateway_multicast_domain_id = domain_id.clone();
        // The caller-chosen bytes a domain holds are the ids of the associations written
        // with it, charged here like every other writer's — on top of the fixed overhead
        // the record itself costs — so no path can store them uncharged and a domain with
        // no association is not free against the byte budget either.
        let association_bytes: usize = record
            .associations
            .iter()
            .map(|association| {
                association.transit_gateway_attachment_id.len()
                    + association.resource_id.len()
                    + association.subnet_id.len()
            })
            .sum();
        self.add_stored_bytes(STORED_RECORD_OVERHEAD_BYTES + association_bytes);
        self.transit_gateway_multicast_domains
            .insert(domain_id.clone(), record);
        Some(domain_id)
    }

    /// Store a transit gateway route table announcement and answer the
    /// `TransitGatewayRouteTableAnnouncementId` it was given — or `None` when the store is
    /// at [`MAX_STORED_RECORDS_PER_STORE`] and nothing was stored.
    pub fn insert_transit_gateway_route_table_announcement(
        &mut self,
        mut view: TransitGatewayRouteTableAnnouncement,
    ) -> Option<String> {
        if !self.has_transit_gateway_route_table_announcement_capacity() {
            return None;
        }
        let announcement_id = self.next_id(TRANSIT_GATEWAY_ROUTE_TABLE_ANNOUNCEMENT_ID_PREFIX);
        view.transit_gateway_route_table_announcement_id = Some(announcement_id.clone());
        self.add_stored_bytes(
            STORED_RECORD_OVERHEAD_BYTES + tag_list_bytes(view.tags.as_deref().unwrap_or_default()),
        );
        self.transit_gateway_route_table_announcements.insert(
            announcement_id.clone(),
            TransitGatewayRouteTableAnnouncementRecord {
                view,
                pending: None,
            },
        );
        Some(announcement_id)
    }

    /// Store one of the account's own VPC endpoint service configurations and answer the
    /// `ServiceId` it was given — or `None` when the store is at
    /// [`MAX_STORED_RECORDS_PER_STORE`] and nothing was stored.
    pub fn insert_vpc_endpoint_service(
        &mut self,
        mut view: ServiceConfiguration,
    ) -> Option<String> {
        if !self.has_vpc_endpoint_service_capacity() {
            return None;
        }
        let service_id = self.next_id(VPC_ENDPOINT_SERVICE_ID_PREFIX);
        view.service_id = Some(service_id.clone());
        self.add_stored_bytes(
            STORED_RECORD_OVERHEAD_BYTES + tag_list_bytes(view.tags.as_deref().unwrap_or_default()),
        );
        self.vpc_endpoint_services.insert(service_id.clone(), view);
        Some(service_id)
    }

    /// Store a VPC endpoint and answer the `VpcEndpointId` it was given — or `None` when
    /// the store is at [`MAX_STORED_RECORDS_PER_STORE`] and nothing was stored.
    pub fn insert_vpc_endpoint(&mut self, mut view: VpcEndpoint) -> Option<String> {
        if !self.has_vpc_endpoint_capacity() {
            return None;
        }
        let endpoint_id = self.next_id(VPC_ENDPOINT_ID_PREFIX);
        view.vpc_endpoint_id = Some(endpoint_id.clone());
        self.add_stored_bytes(
            STORED_RECORD_OVERHEAD_BYTES + tag_list_bytes(view.tags.as_deref().unwrap_or_default()),
        );
        self.vpc_endpoints.insert(endpoint_id.clone(), view);
        Some(endpoint_id)
    }

    /// Store an IPAM pool and answer the `IpamPoolId` it was given — or `None` when the
    /// store is at [`MAX_STORED_RECORDS_PER_STORE`] and nothing was stored.
    pub fn insert_ipam_pool(&mut self, mut view: IpamPool) -> Option<String> {
        if !self.has_ipam_pool_capacity() {
            return None;
        }
        let pool_id = self.next_id(IPAM_POOL_ID_PREFIX);
        view.ipam_pool_id = Some(pool_id.clone());
        view.ipam_pool_arn = Some(::engine::emulator_arn(
            "ec2",
            view.ipam_region.as_deref().unwrap_or(DEFAULT_REGION),
            &format!("ipam-pool/{pool_id}"),
        ));
        self.add_stored_bytes(
            STORED_RECORD_OVERHEAD_BYTES
                + tag_list_bytes(view.tags.as_deref().unwrap_or_default())
                + view.description.as_deref().unwrap_or_default().len(),
        );
        self.ipam_pools.insert(pool_id.clone(), view);
        Some(pool_id)
    }
}

/// The fixed overhead one stored VPC endpoint connection costs whatever it carries: the
/// modelled record itself and the entry holding it, as a CHOSEN constant in the same
/// spirit as the count ceilings — the point is that a zero-tag connection is not free
/// against [`MAX_STORED_BYTES`], not that it is byte-exact.
pub const VPC_ENDPOINT_CONNECTION_RECORD_BYTES: usize = 512;

// =============================================================================
// Amazon S3 storage locations a request names
// =============================================================================

/// The code EC2 answers when it cannot read the Amazon S3 location a request named — the
/// bucket is not one it can reach at all.
pub const INACCESSIBLE_STORAGE_LOCATION: &str = "InaccessibleStorageLocation";

/// The shortest name a general-purpose Amazon S3 bucket may have: "Bucket names must be
/// between 3 (min) and 63 (max) characters long" (Amazon S3 User Guide, "General purpose
/// bucket naming rules").
pub const MIN_S3_BUCKET_NAME_CHARS: usize = 3;

/// The longest name a general-purpose Amazon S3 bucket may have (see
/// [`MIN_S3_BUCKET_NAME_CHARS`]).
pub const MAX_S3_BUCKET_NAME_CHARS: usize = 63;

/// The prefixes a general-purpose bucket name must not start with: "Bucket names must not
/// start with the prefix `xn--`. … must not start with the prefix `sthree-`. … must not
/// start with the prefix `amzn-s3-demo-`" (Amazon S3 User Guide, "General purpose bucket
/// naming rules").
pub const S3_BUCKET_NAME_RESERVED_PREFIXES: [&str; 3] = ["xn--", "sthree-", "amzn-s3-demo-"];

/// The suffixes a general-purpose bucket name must not end with: "Bucket names must not end
/// with the suffix `-s3alias`. … must not end with the suffix `--ol-s3`. … must not end with
/// the suffix `.mrap`. … must not end with the suffix `--x-s3`. … must not end with the
/// suffix `--table-s3`" (Amazon S3 User Guide, "General purpose bucket naming rules").
pub const S3_BUCKET_NAME_RESERVED_SUFFIXES: [&str; 5] =
    ["-s3alias", "--ol-s3", ".mrap", "--x-s3", "--table-s3"];

/// Is `name` a legal general-purpose Amazon S3 bucket name?
///
/// Every rule below is one sentence of the Amazon S3 User Guide's "General purpose bucket
/// naming rules": the length ([`MIN_S3_BUCKET_NAME_CHARS`]..=[`MAX_S3_BUCKET_NAME_CHARS`]),
/// "can consist only of lowercase letters, numbers, dots (.), and hyphens (-)", "must begin
/// and end with a letter or number", "must not contain two adjacent periods", "must not be
/// formatted as an IP address (for example, 192.168.5.4)", and the reserved prefixes and
/// suffixes above. A value that breaks any of them names no bucket at all, so no service can
/// read an object out of it.
pub fn s3_bucket_name_is_well_formed(name: &str) -> bool {
    // The character set FIRST, so the length rule below counts characters of a name that
    // can only be ASCII — "can consist only of lowercase letters, numbers, dots (.), and
    // hyphens (-)" — and a byte count is that character count.
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'.')
    {
        return false;
    }
    if !(MIN_S3_BUCKET_NAME_CHARS..=MAX_S3_BUCKET_NAME_CHARS).contains(&name.len()) {
        return false;
    }
    let begins_and_ends_alphanumeric = name
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && name
            .bytes()
            .next_back()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit());
    if !begins_and_ends_alphanumeric {
        return false;
    }
    if name.contains("..") {
        return false;
    }
    // "Must not be formatted as an IP address": four dot-separated decimal octets.
    if name.parse::<::std::net::Ipv4Addr>().is_ok() {
        return false;
    }
    if S3_BUCKET_NAME_RESERVED_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
    {
        return false;
    }
    !S3_BUCKET_NAME_RESERVED_SUFFIXES
        .iter()
        .any(|suffix| name.ends_with(suffix))
}

/// Refuse a request whose Amazon S3 storage location this service cannot read: HTTP 400
/// `InaccessibleStorageLocation`.
pub fn inaccessible_storage_location<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(INACCESSIBLE_STORAGE_LOCATION, 400, message)
}

/// The bucket and key of a storage location a request named, refused the way EC2 refuses
/// one it cannot read.
///
/// `member` names the request member the location came from, so the message says which one
/// was wrong. A location that names no bucket or no key names no object — there is nothing
/// to read — and a bucket name outside [`s3_bucket_name_is_well_formed`] names no bucket at
/// all, which is [`INACCESSIBLE_STORAGE_LOCATION`]. The bucket is checked BEFORE the key,
/// because a name that is not a bucket name is already enough to answer with.
pub fn readable_storage_location<'a, E>(
    location: &'a StorageLocation,
    member: &str,
) -> Result<(&'a str, &'a str), Scaffolded<E>> {
    let bucket = location
        .bucket
        .as_deref()
        .filter(|bucket| !bucket.is_empty())
        .ok_or_else(|| missing_parameter(format!("{member} must name an S3 bucket")))?;
    if !s3_bucket_name_is_well_formed(bucket) {
        return Err(inaccessible_storage_location(format!(
            "The S3 bucket '{bucket}' named by {member} is not an accessible storage \
             location: an S3 bucket name holds {MIN_S3_BUCKET_NAME_CHARS} to \
             {MAX_S3_BUCKET_NAME_CHARS} lowercase letters, digits, dots and hyphens, \
             begins and ends with a letter or a digit, and is not formatted as an IP address"
        )));
    }
    let key = location
        .key
        .as_deref()
        .filter(|key| !key.is_empty())
        .ok_or_else(|| missing_parameter(format!("{member} must name an S3 object key")))?;
    Ok((bucket, key))
}

// =============================================================================
// Amazon FPGA Images (AFIs)
// =============================================================================

/// An AFI whose creation is still running: `FpgaImageStateCode`'s `pending`
/// (`crate::ops::types::FPGA_IMAGE_STATE_CODE_VALUES` is the generated catalog).
pub const FPGA_IMAGE_STATE_PENDING: &str = "pending";

/// An AFI whose creation did not produce a bitstream: `FpgaImageStateCode`'s `failed`.
pub const FPGA_IMAGE_STATE_FAILED: &str = "failed";

/// The id prefix of an Amazon FPGA Image: `afi-`.
pub const FPGA_IMAGE_ID_PREFIX: &str = "afi";

/// The one resource type `CreateFpgaImage`'s `TagSpecifications` can name: "The tags to
/// apply to the FPGA image during creation"
/// (`CreateFpgaImageRequest$TagSpecifications`), which is the `fpga-image` member of
/// `ResourceType`.
pub const FPGA_IMAGE_TAGGABLE_RESOURCE: &str = "fpga-image";

/// The resource types a restore of an AMI from Amazon S3 may tag: "You can tag the AMI, the
/// snapshots, or both. To tag the AMI, the value for `ResourceType` must be `image`. To tag
/// the snapshots, the value for `ResourceType` must be `snapshot`"
/// (`CreateRestoreImageTaskRequest$TagSpecifications`) — the `image` and `snapshot` members
/// of `ResourceType`.
pub const IMAGE_TAGGABLE_RESOURCES: [&str; 2] = ["image", "snapshot"];

/// The id prefix of the global identifier of an Amazon FPGA Image: `agfi-`
/// ("The global FPGA image identifier (AGFI ID)", `CreateFpgaImageResult$FpgaImageGlobalId`).
pub const FPGA_IMAGE_GLOBAL_ID_PREFIX: &str = "agfi";

/// The code EC2 answers for something that is not an AFI id at all.
pub const INVALID_FPGA_IMAGE_ID_MALFORMED: &str = "InvalidFpgaImageID.Malformed";

/// The code EC2 answers for an AFI id the account holds no FPGA image for.
pub const INVALID_FPGA_IMAGE_ID_NOT_FOUND: &str = "InvalidFpgaImageID.NotFound";

/// EC2's own code for a request that would take the account past a resource limit this
/// service answers no resource-specific code for — "You have exceeded an Amazon EC2
/// resource limit" (EC2 API error codes, `ResourceLimitExceeded`).
pub const RESOURCE_LIMIT_EXCEEDED: &str = "ResourceLimitExceeded";

/// How many Amazon FPGA Images this emulator stores before `CreateFpgaImage` is refused.
///
/// CHOSEN, like [`MAX_STORED_VOLUMES`] and for the same reason: the store lives in this
/// process's memory, so the number of AFIs a caller can create has to be finite. It is well
/// above the published per-Region AFI quota (10 by default, which is adjustable), so a test
/// that creates more than a real account's default quota is not refused for a reason this
/// emulator cannot have.
pub const MAX_FPGA_IMAGES: usize = 1_000;

/// Refuse a request that would take the store past a ceiling with no resource-specific code
/// of its own: HTTP 400 [`RESOURCE_LIMIT_EXCEEDED`].
pub fn resource_limit_exceeded<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(RESOURCE_LIMIT_EXCEEDED, 400, message)
}

/// Is `id` shaped like an AFI id?
///
/// `afi-` followed by 8 or 17 lowercase hexadecimal digits — the short and the long id form,
/// the same two forms [`volume_id_is_well_formed`] and [`image_id_is_well_formed`] accept,
/// read by the one [`prefixed_id_is_well_formed`] every resource of this module asks.
/// EC2 answers `InvalidFpgaImageID.Malformed` for anything else, BEFORE it looks the id up.
pub fn fpga_image_id_is_well_formed(id: &str) -> bool {
    prefixed_id_is_well_formed(FPGA_IMAGE_ID_PREFIX, id)
}

/// Refuse a request that named an AFI id the account holds no FPGA image for: HTTP 400
/// `InvalidFpgaImageID.NotFound`.
pub fn invalid_fpga_image_id_not_found<E>(fpga_image_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_FPGA_IMAGE_ID_NOT_FOUND,
        400,
        format!("The FPGA image id '{fpga_image_id}' does not exist"),
    )
}

/// Refuse an AFI id the request named that is not an AFI id at all: HTTP 400
/// `InvalidFpgaImageID.Malformed`, which EC2 answers BEFORE it looks the id up.
///
/// The pair of this and [`invalid_fpga_image_id_not_found`] is the ONE reading of the
/// malformed-before-not-found order for an AFI; they are two functions rather than one
/// `require_known_…` because every operation that takes an AFI id answers its `DryRun`
/// probe between them.
pub fn invalid_fpga_image_id_malformed<E>(fpga_image_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_FPGA_IMAGE_ID_MALFORMED,
        400,
        format!("Invalid id: '{fpga_image_id}'"),
    )
}

/// One stored Amazon FPGA Image: the modelled [`FpgaImage`] every operation that reports one
/// answers with, its load permissions, and the move the clock owes its creation.
///
/// The view holds no `State`: an AFI's state is a fact of how far its creation has got, so
/// [`FpgaImageRecord::view_at`] derives it from the stored due instant rather than letting
/// two writers store two different words for it — the same way [`VolumeRecord`] treats a
/// volume's state.
///
/// "The create operation is asynchronous. To verify that the AFI was successfully created
/// and is ready for use, check the output logs" (`CreateFpgaImage`): the call hands back a
/// `pending` AFI, and the outcome of the creation is reported in the AFI's own state. This
/// emulator stores no Amazon S3 objects, so nothing can read the design checkpoint the
/// request named and no bitstream is ever produced — the creation therefore ends in the
/// modelled `failed` state carrying [`FpgaImageRecord::failure_message`], which says so,
/// rather than in an `available` AFI nothing built.
#[derive(Debug, Clone)]
pub struct FpgaImageRecord {
    /// The AFI as the model shapes it, with `State` left for [`FpgaImageRecord::view_at`].
    pub view: FpgaImage,
    /// "The load permissions" (`FpgaImageAttribute$LoadPermissions`) of this AFI: which
    /// accounts and groups may load it onto an FPGA. `ModifyFpgaImageAttribute` adds and
    /// removes them and `ResetFpgaImageAttribute` resets them to the default, which is none
    /// — an AFI starts loadable by its owner only.
    pub load_permissions: Vec<LoadPermission>,
    /// The instant from which this AFI's `pending` -> `failed` move is due. Monotonic
    /// ([`Instant`]) for the same reason [`VolumeRecord::available_at`] is.
    pub resolved_at: Instant,
    /// Why the creation failed, reported as `FpgaImageState$Message` once the move above is
    /// due. Built from the request's own storage location, never from a recording.
    pub failure_message: String,
    /// The caller-chosen bytes this record was charged against [`MAX_STORED_BYTES`] when it
    /// was stored ([`fpga_image_payload_bytes`]), so the removal path gives back exactly
    /// what the create charged.
    pub payload_bytes: usize,
}

impl FpgaImageRecord {
    /// This AFI as of `now`: the stored view with the `State` the clock has reached.
    pub fn view_at(&self, now: Instant) -> FpgaImage {
        let mut view = self.view.clone();
        view.state = Some(self.state_at(now));
        view
    }

    /// The AFI's `FpgaImageState` at `now`.
    pub fn state_at(&self, now: Instant) -> FpgaImageState {
        if now < self.resolved_at {
            FpgaImageState {
                code: Some(FPGA_IMAGE_STATE_PENDING.to_string()),
                message: None,
            }
        } else {
            FpgaImageState {
                code: Some(FPGA_IMAGE_STATE_FAILED.to_string()),
                message: Some(self.failure_message.clone()),
            }
        }
    }
}

/// The caller-chosen bytes one stored AFI holds: its tags, the name and description the
/// request gave it, the storage locations it named and the `ClientToken` the token map keeps
/// for it. Everything else on the record — the ids, the account, the states and the
/// timestamps — is derived by this module, so it is fixed-size.
///
/// The ONE definition of that charge, the way [`volume_payload_bytes`] is for a volume: a
/// writer charges exactly it, and a removal path gives back the same number.
pub fn fpga_image_payload_bytes(
    tags: &[Tag],
    name: Option<&str>,
    description: Option<&str>,
    locations: &[&str],
    client_token: Option<&str>,
) -> usize {
    tag_list_bytes(tags)
        .saturating_add(name.map_or(0, str::len))
        .saturating_add(description.map_or(0, str::len))
        .saturating_add(locations.iter().map(|value| value.len()).sum())
        .saturating_add(client_token.map_or(0, str::len))
}

impl ComAmazonawsEc2Amazonec2State {
    /// Take the AFI `fpga_image_id` out of the store, and answer the record as it was last
    /// stored; `None` when the store holds no such AFI.
    ///
    /// The bytes the record was charged are given back and any `ClientToken` that named it
    /// is dropped, so a token can never name an AFI that is gone — the same contract
    /// [`Self::remove_launch_template`] keeps for a launch template.
    pub fn remove_fpga_image(&mut self, fpga_image_id: &str) -> Option<FpgaImageRecord> {
        let record = self.fpga_images.remove(fpga_image_id)?;
        self.fpga_image_client_tokens
            .retain(|_, minted| minted != fpga_image_id);
        self.release_stored_bytes(record.payload_bytes);
        Some(record)
    }
}

// =============================================================================
// AMI watermarks
// =============================================================================

/// The characters a `WatermarkName` may carry beside letters and digits: "Constraints: 3-128
/// alphanumeric characters, parentheses (()), square brackets ([]), spaces ( ), periods (.),
/// slashes (/), dashes (-), single quotes ('), at-signs (@), or underscores(_)"
/// (`AttachImageWatermarkRequest$WatermarkName`). The LENGTH of that sentence is modelled
/// (`@length(min: 3, max: 128)`) and refused before a handler runs; the character set is
/// not, so a handler is the only place it can be enforced.
pub const WATERMARK_NAME_PUNCTUATION: &str = "()[] ./-'@_";

/// How many watermarks one AMI carries before an attach is refused.
///
/// CHOSEN: AWS publishes no quota for watermarks per AMI, and each attach retains the
/// caller's watermark name for the process's lifetime, so the list needs a ceiling that is
/// not the process's memory. The bytes each watermark retains are charged against
/// [`MAX_STORED_BYTES`] as well.
pub const MAX_WATERMARKS_PER_IMAGE: usize = 50;

/// Is `name` a well-formed `WatermarkName` — does it carry only the characters
/// [`WATERMARK_NAME_PUNCTUATION`]'s sentence allows?
pub fn watermark_name_is_well_formed(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || WATERMARK_NAME_PUNCTUATION.contains(c))
}

/// The `WatermarkKey` a watermark named `watermark_name` attached by `account` carries: "The
/// watermark identifier, in `accountId:watermarkName` format (for example,
/// `123456789012:approvedAmi`). The `accountId` portion is the Amazon Web Services account
/// ID of the watermark creator. The `watermarkName` portion is customer-provided"
/// (`ImageWatermark$WatermarkKey`).
pub fn image_watermark_key(account: &str, watermark_name: &str) -> String {
    format!("{account}:{watermark_name}")
}

// =============================================================================
// The Recycle Bin
// =============================================================================

/// A volume that has been deleted: `VolumeState`'s `deleted`, which is what a volume held in
/// the Recycle Bin reports — it has been deleted, and is kept only until its retention
/// period runs out.
pub const VOLUME_STATE_DELETED: &str = "deleted";

/// The page sizes a report of the Recycle Bin's volumes accepts: "Valid range: 5 - 500"
/// (`ListVolumesInRecycleBinRequest$MaxResults`). A request that names none gets the top of
/// this range, so the `NextToken` path behaves the same whether or not a page size was asked
/// for.
pub const RECYCLE_BIN_PAGE_SIZE_RANGE: (i32, i32) = (5, 500);

/// The bytes one entry of the Recycle Bin holds.
///
/// The ONE definition of that charge, the way [`volume_payload_bytes`] is for a volume: the
/// fixed [`STORED_RECORD_OVERHEAD_BYTES`] every stored record costs — so an entry carrying
/// no optional text is NOT free against [`MAX_STORED_BYTES`] — plus the text the entry
/// carries over from the volume it was derived from, which is text a request chose.
pub fn volume_recycle_bin_entry_payload_bytes(entry: &VolumeRecycleBinInfo) -> usize {
    STORED_RECORD_OVERHEAD_BYTES
        .saturating_add(entry.volume_type.as_deref().map_or(0, str::len))
        .saturating_add(entry.outpost_arn.as_deref().map_or(0, str::len))
        .saturating_add(entry.snapshot_id.as_deref().map_or(0, str::len))
        .saturating_add(entry.source_volume_id.as_deref().map_or(0, str::len))
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another entry fits inside the Recycle Bin's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]).
    ///
    /// The same contract as [`Self::has_transit_gateway_attachment_capacity`]: a caller asks
    /// this BEFORE moving anything and refuses the request with EC2's own quota code when it
    /// answers `false`, and [`Self::move_volume_to_recycle_bin`] asks it again for itself, so
    /// no path can get past the ceiling by forgetting to. The ceiling is needed because
    /// nothing in this emulator reclaims an entry of the bin, so without it a sequence of
    /// create-then-delete requests would grow memory without bound.
    pub fn has_volume_recycle_bin_capacity(&self) -> bool {
        self.volumes_in_recycle_bin.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Move the stored volume `volume_id` into the Recycle Bin, where it stays for
    /// `retention`, and answer the entry a report of the bin now holds for it.
    ///
    /// The entry is DERIVED from the stored
    /// volume rather than taken from a request, so a volume's size, type, zone and
    /// provisioning cannot read differently inside the bin from outside it; `retention` is
    /// the retention period of the Recycle Bin rule that covered the volume, which is a fact
    /// of that rule (Amazon Recycle Bin, "Retention rules") and so is given by the caller.
    ///
    /// `None` when the store holds no such volume, when the bin is at
    /// [`MAX_STORED_RECORDS_PER_STORE`], or when `now + retention` is not a representable
    /// instant. In each case NOTHING was removed and nothing stored — every fallible step is
    /// taken before the first mutation — so a refused move leaves the volume where it was
    /// instead of losing it between the two stores.
    ///
    /// The byte accounting is symmetric, the way [`Self::remove_fpga_image`] is: the bytes
    /// the create charged for the volume are given back as it leaves the store, and the
    /// entry that takes its place is charged for itself
    /// ([`volume_recycle_bin_entry_payload_bytes`]), so a repeated create-then-delete
    /// sequence advances the byte budget rather than trading a charged volume for a free
    /// entry.
    pub fn move_volume_to_recycle_bin(
        &mut self,
        volume_id: &str,
        now: SystemTime,
        retention: Duration,
    ) -> Option<VolumeRecycleBinInfo> {
        if !self.has_volume_recycle_bin_capacity() {
            return None;
        }
        // "The date and time when the volume is to be permanently deleted from the Recycle
        // Bin" is `now` plus the retention period the rule carries, and `SystemTime + Duration`
        // PANICS when that instant is not representable. The retention period comes from the
        // caller, so the sum is asked for with `checked_add` and the move is refused rather
        // than unwinding — and it is asked BEFORE the volume is removed.
        let exit_at = now.checked_add(retention)?;
        let record = self.volumes.remove(volume_id)?;
        let client_token = self
            .volume_client_tokens
            .iter()
            .find(|(_, minted)| minted.as_str() == volume_id)
            .map(|(token, _)| token.clone());
        if let Some(token) = &client_token {
            self.volume_client_tokens.remove(token);
        }
        let view = record.view;
        // Give back exactly what `CreateVolume` charged for this volume, the way
        // [`Self::remove_fpga_image`] does for an FPGA image: the byte budget stays the sum of
        // what the store actually holds, and the entry stored below is charged for itself.
        self.release_stored_bytes(volume_payload_bytes(
            view.tags.as_deref().unwrap_or_default(),
            view.kms_key_id.as_deref(),
            client_token.as_deref(),
        ));
        let entry = VolumeRecycleBinInfo {
            volume_id: view.volume_id.clone(),
            volume_type: view.volume_type.clone(),
            // "The state of the volume" (`VolumeRecycleBinInfo$State`): a volume in the
            // Recycle Bin has been deleted and is kept only until its retention period runs
            // out, which is `VolumeState`'s `deleted`.
            state: Some(VOLUME_STATE_DELETED.to_string()),
            size: view.size,
            iops: view.iops,
            throughput: view.throughput,
            outpost_arn: view.outpost_arn.clone(),
            availability_zone: view.availability_zone.clone(),
            availability_zone_id: view.availability_zone_id.clone(),
            source_volume_id: view.source_volume_id.clone(),
            snapshot_id: view.snapshot_id.clone(),
            operator: view.operator.clone(),
            create_time: view.create_time,
            // "The date and time when the volume entered the Recycle Bin" — now — and "the
            // date and time when the volume is to be permanently deleted from the Recycle
            // Bin", which is when the retention period that covered it runs out.
            recycle_bin_enter_time: Some(system_time_to_date_time(now)),
            recycle_bin_exit_time: Some(system_time_to_date_time(exit_at)),
        };
        self.add_stored_bytes(volume_recycle_bin_entry_payload_bytes(&entry));
        self.volumes_in_recycle_bin
            .insert(volume_id.to_string(), entry.clone());
        Some(entry)
    }
}

// =============================================================================
// AMI usage reports
// =============================================================================

/// The code EC2 answers for a report id the account holds no AMI usage report for.
///
/// EC2 names a resource a request could not resolve `Invalid<Resource>Id.NotFound` — as
/// [`INVALID_LAUNCH_TEMPLATE_ID_NOT_FOUND`] and [`INVALID_FPGA_IMAGE_ID_NOT_FOUND`] read —
/// so a report id that names nothing is answered the same way.
pub const INVALID_IMAGE_USAGE_REPORT_ID_NOT_FOUND: &str = "InvalidImageUsageReportId.NotFound";

/// Refuse a request that named a report id the account holds no AMI usage report for: HTTP
/// 400 `InvalidImageUsageReportId.NotFound`.
pub fn invalid_image_usage_report_id_not_found<E>(report_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_IMAGE_USAGE_REPORT_ID_NOT_FOUND,
        400,
        format!("The image usage report id '{report_id}' does not exist"),
    )
}

// =============================================================================
// Instance export tasks
// =============================================================================

/// The code EC2 answers for an export task id the account holds no export task for.
pub const INVALID_EXPORT_TASK_ID_NOT_FOUND: &str = "InvalidExportTaskID.NotFound";

/// An export task that is still running: `ExportTaskState`'s `active`
/// (`crate::ops::types::EXPORT_TASK_STATE_VALUES` is the generated catalog).
pub const EXPORT_TASK_STATE_ACTIVE: &str = "active";

/// An export task a cancel has started taking down: `ExportTaskState`'s `cancelling`.
pub const EXPORT_TASK_STATE_CANCELLING: &str = "cancelling";

/// An export task whose cancellation is done: `ExportTaskState`'s `cancelled`.
pub const EXPORT_TASK_STATE_CANCELLED: &str = "cancelled";

/// An export task that finished: `ExportTaskState`'s `completed`.
pub const EXPORT_TASK_STATE_COMPLETED: &str = "completed";

/// One stored instance export task: the modelled [`ExportTask`] every operation that reports
/// one answers with, plus the move the clock owes a cancellation.
///
/// "Cancels an active export task" (`CancelExportTask`): a cancel puts the task into the
/// non-terminal `cancelling` state, so the clock owes it `cancelled` [`TRANSITION_DELAY`]
/// later whether or not anybody calls again. The due instant is stored and
/// [`ExportTaskRecord::view_at`] derives the state from it, the way [`VolumeRecord`] does
/// for a volume.
#[derive(Debug, Clone)]
pub struct ExportTaskRecord {
    /// The export task as the model shapes it, carrying the state it has reached on its own.
    pub view: ExportTask,
    /// When the `cancelling` -> `cancelled` move of a cancelled task falls due; `None` for a
    /// task nothing has cancelled.
    pub cancelled_at: Option<Instant>,
}

impl ExportTaskRecord {
    /// This export task as of `now`: the stored view with the state the clock has reached.
    pub fn view_at(&self, now: Instant) -> ExportTask {
        let mut view = self.view.clone();
        if let Some(state) = self.cancellation_state_at(now) {
            view.state = Some(state.to_string());
        }
        view
    }

    /// The state a cancelled task has reached at `now`, or `None` for a task nothing has
    /// cancelled — whose state is the one the task itself carries.
    fn cancellation_state_at(&self, now: Instant) -> Option<&'static str> {
        self.cancelled_at.map(|due_at| {
            if now >= due_at {
                EXPORT_TASK_STATE_CANCELLED
            } else {
                EXPORT_TASK_STATE_CANCELLING
            }
        })
    }

    /// The task's `ExportTaskState` at `now`.
    pub fn state_at(&self, now: Instant) -> String {
        self.cancellation_state_at(now)
            .map(str::to_string)
            .or_else(|| self.view.state.clone())
            .unwrap_or_else(|| EXPORT_TASK_STATE_ACTIVE.to_string())
    }
}

/// Refuse a request that named an export task id the account holds no export task for: HTTP
/// 400 `InvalidExportTaskID.NotFound`.
pub fn invalid_export_task_id_not_found<E>(export_task_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_EXPORT_TASK_ID_NOT_FOUND,
        400,
        format!("The export task id '{export_task_id}' does not exist"),
    )
}

// ---------------------------------------------------------------------------
// Block public access for snapshots
// ---------------------------------------------------------------------------

/// "`unblocked` - Public sharing is not blocked. Users can publicly share snapshots"
/// (`GetSnapshotBlockPublicAccessStateResult$State`): the `SnapshotBlockPublicAccessState`
/// enum's `unblocked` member.
///
/// It is both where an account that has never enabled the setting sits and the state
/// `DisableSnapshotBlockPublicAccess` returns it to — "after you disable block public
/// access for snapshots in a Region, users can publicly share snapshots in that Region".
pub const SNAPSHOT_BLOCK_PUBLIC_ACCESS_UNBLOCKED: &str = "unblocked";

/// "`block-all-sharing` - Prevents all public sharing of snapshots in the Region"
/// (`EnableSnapshotBlockPublicAccessRequest$State`): the `SnapshotBlockPublicAccessState`
/// enum's `block-all-sharing` member.
pub const SNAPSHOT_BLOCK_PUBLIC_ACCESS_BLOCK_ALL_SHARING: &str = "block-all-sharing";

/// "`block-new-sharing` - Prevents only new public sharing of snapshots in the Region"
/// (`EnableSnapshotBlockPublicAccessRequest$State`): the `SnapshotBlockPublicAccessState`
/// enum's `block-new-sharing` member.
pub const SNAPSHOT_BLOCK_PUBLIC_ACCESS_BLOCK_NEW_SHARING: &str = "block-new-sharing";

/// The modes `EnableSnapshotBlockPublicAccess` accepts: the two blocking members of the
/// `SnapshotBlockPublicAccessState` catalog, in the order its own documentation lists
/// them.
///
/// The third member is NOT here on purpose: "`unblocked` is not a valid value for
/// **EnableSnapshotBlockPublicAccess**" (`EnableSnapshotBlockPublicAccessRequest$State`) —
/// turning the setting off is `DisableSnapshotBlockPublicAccess`'s job. The whole catalog
/// is the generated `crate::ops::types::SNAPSHOT_BLOCK_PUBLIC_ACCESS_STATE_VALUES`.
pub const SNAPSHOT_BLOCK_PUBLIC_ACCESS_MODES: [&str; 2] = [
    SNAPSHOT_BLOCK_PUBLIC_ACCESS_BLOCK_ALL_SHARING,
    SNAPSHOT_BLOCK_PUBLIC_ACCESS_BLOCK_NEW_SHARING,
];

/// "`account` - The state is managed by the account"
/// (`GetSnapshotBlockPublicAccessStateResult$ManagedBy`): the `ManagedBy` enum's `account`
/// member.
///
/// Every state this emulator can hold is one the account itself set through
/// `EnableSnapshotBlockPublicAccess` / `DisableSnapshotBlockPublicAccess`, or the default
/// it has never changed. The enum's other member, `declarative-policy`, names a state set
/// OUTSIDE EC2 — by an Organizations declarative policy — and "can't be modified by the
/// account", so no operation of this API can put the setting into it.
pub const MANAGED_BY_ACCOUNT: &str = "account";

/// The [`SNAPSHOT_BLOCK_PUBLIC_ACCESS_MODES`] member `requested` names, or `None` when it
/// names none.
///
/// Answering a `&'static str` out of this module's own constants is what keeps request
/// text out of the store: a writer stores what this returns, so the stored word is always
/// a modelled catalog member.
pub fn snapshot_block_public_access_mode(requested: &str) -> Option<&'static str> {
    SNAPSHOT_BLOCK_PUBLIC_ACCESS_MODES
        .into_iter()
        .find(|mode| *mode == requested)
}

impl ComAmazonawsEc2Amazonec2State {
    /// The state of the account's *block public access for snapshots* setting right now.
    ///
    /// A pure read: the setting has no clock-owed move — AWS reports the mode an enable
    /// asked for as soon as the enable answers — so nothing about it changes with time.
    pub fn snapshot_block_public_access_state(&self) -> &'static str {
        self.snapshot_block_public_access
    }

    /// Put the setting into `state` and answer the state now in effect.
    ///
    /// `state` is a `&'static str` so only one of this module's catalog constants can be
    /// stored; the answer is read back off the field rather than echoed, so a writer
    /// reports exactly what a later read will.
    pub fn set_snapshot_block_public_access_state(&mut self, state: &'static str) -> &'static str {
        self.snapshot_block_public_access = state;
        self.snapshot_block_public_access
    }
}

// ---------------------------------------------------------------------------
// Secondary subnets
// ---------------------------------------------------------------------------

/// EC2's error code for a request that named a secondary subnet id no secondary subnet of
/// the account has.
///
/// EC2 spells an id it cannot resolve `Invalid<Resource>ID.NotFound` —
/// [`INVALID_SUBNET_ID_NOT_FOUND`], [`INVALID_VPC_ID_NOT_FOUND`],
/// [`INVALID_TRANSIT_GATEWAY_ID_NOT_FOUND`] — so the secondary subnet's own code follows
/// that form. EC2 models no error shapes at all, so it is answered through
/// [`crate::scaffolded::Scaffolded::service_error`] like every other code here.
pub const INVALID_SECONDARY_SUBNET_ID_NOT_FOUND: &str = "InvalidSecondarySubnetID.NotFound";

/// Refuse a request that named a secondary subnet id no secondary subnet of the account
/// has, the way [`invalid_subnet_id_not_found`] refuses an unknown subnet id: HTTP 400.
pub fn invalid_secondary_subnet_id_not_found<E>(secondary_subnet_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_SECONDARY_SUBNET_ID_NOT_FOUND,
        400,
        format!("The secondary subnet ID '{secondary_subnet_id}' does not exist"),
    )
}

/// The id prefix a secondary subnet carries.
///
/// CHOSEN: the model publishes no `@pattern` for `SecondarySubnetId`
/// (`smithy_query(query="shape",shape="SecondarySubnetId")` reports a plain string), so the
/// only form it has to satisfy is EC2's own `<prefix>-<hex>`, which
/// [`ComAmazonawsEc2Amazonec2State::next_id`] gives.
pub const SECONDARY_SUBNET_ID_PREFIX: &str = "snet";

/// The id prefix of one IPv4 CIDR block association of a secondary subnet, on the same
/// terms as [`SECONDARY_SUBNET_ID_PREFIX`]: `SecondarySubnetCidrAssociationId` is a plain
/// string in the model, and an ordinary subnet's CIDR associations are minted under
/// `subnet-cidr-assoc`.
pub const SECONDARY_SUBNET_CIDR_ASSOCIATION_ID_PREFIX: &str = "snet-cidr-assoc";

/// The only secondary network type the model publishes: `SecondaryNetworkType`'s sole
/// member — "The type of the secondary network (`rdma`)"
/// (`DescribeSecondarySubnetsRequest$Filters`). The whole catalog is the generated
/// [`crate::ops::types::SECONDARY_NETWORK_TYPE_VALUES`].
pub const SECONDARY_NETWORK_TYPE_RDMA: &str = "rdma";

/// A secondary subnet that has been created: `SecondarySubnetState`'s `create-complete`
/// member, the state a usable secondary subnet sits in. The whole catalog is the generated
/// [`crate::ops::types::SECONDARY_SUBNET_STATE_VALUES`]; the states a creation or a
/// deletion passes through belong to the operations that perform them.
pub const SECONDARY_SUBNET_STATE_CREATE_COMPLETE: &str = "create-complete";

/// An IPv4 CIDR block that is associated with its secondary subnet:
/// `SecondarySubnetCidrBlockAssociationState`'s `associated` member. The whole catalog is
/// the generated [`crate::ops::types::SECONDARY_SUBNET_CIDR_BLOCK_ASSOCIATION_STATE_VALUES`].
pub const SECONDARY_SUBNET_CIDR_STATE_ASSOCIATED: &str = "associated";

/// One IPv4 CIDR block of a secondary subnet, as the writer of one asks for it.
///
/// The association id is NOT here: it is minted by
/// [`ComAmazonawsEc2Amazonec2State::insert_secondary_subnet`], the way every other stored
/// id is, so no writer can choose one.
#[derive(Debug, Clone)]
pub struct SecondarySubnetCidrSpec {
    /// "The IPv4 CIDR block" (`SecondarySubnetIpv4CidrBlockAssociation$CidrBlock`).
    pub cidr_block: String,
    /// The state of the association, as the model's
    /// `SecondarySubnetCidrBlockAssociationState` enum spells it — see
    /// [`SECONDARY_SUBNET_CIDR_STATE_ASSOCIATED`].
    pub state: String,
    /// "The reason for the current state of the CIDR block association"
    /// (`SecondarySubnetIpv4CidrBlockAssociation$StateReason`), or `None` for an
    /// association that carries none.
    pub state_reason: Option<String>,
}

/// What one stored secondary subnet is created with, beside the ids the store mints itself.
///
/// The spec of the resource, in the same shape [`SubnetSpec`] has for an ordinary subnet: a
/// writer fills this in and [`ComAmazonawsEc2Amazonec2State::insert_secondary_subnet`]
/// turns it into the modelled [`SecondarySubnet`] the describe reports, so the ids, the
/// ARN, the owning account and the byte charge are decided in exactly one place.
#[derive(Debug, Clone)]
pub struct SecondarySubnetSpec {
    /// "The ID of the secondary network" the subnet belongs to
    /// (`SecondarySubnet$SecondaryNetworkId`).
    pub secondary_network_id: String,
    /// "The type of the secondary network" (`SecondarySubnet$SecondaryNetworkType`), as the
    /// model's `SecondaryNetworkType` enum spells it — see
    /// [`SECONDARY_NETWORK_TYPE_RDMA`].
    pub secondary_network_type: String,
    /// "The Availability Zone of the secondary subnet"
    /// (`SecondarySubnet$AvailabilityZone`).
    pub availability_zone: String,
    /// "The ID of the Availability Zone of the secondary subnet"
    /// (`SecondarySubnet$AvailabilityZoneId`).
    pub availability_zone_id: String,
    /// The Region the subnet's ARN names. Not a member of the shape: it is what
    /// `engine::emulator_arn` needs to build `SecondarySubnetArn`.
    pub region: String,
    /// The IPv4 CIDR blocks associated with the subnet, in the order they are reported.
    pub ipv4_cidr_blocks: Vec<SecondarySubnetCidrSpec>,
    /// The subnet's state, as the model's `SecondarySubnetState` enum spells it — see
    /// [`SECONDARY_SUBNET_STATE_CREATE_COMPLETE`].
    pub state: String,
    /// "The reason for the current state of the secondary subnet"
    /// (`SecondarySubnet$StateReason`), or `None` for a subnet that carries none.
    pub state_reason: Option<String>,
    /// "The tags assigned to the secondary subnet" (`SecondarySubnet$Tags`).
    pub tags: Vec<Tag>,
}

impl ComAmazonawsEc2Amazonec2State {
    /// Is there room for another secondary subnet in this store
    /// ([`MAX_STORED_SECONDARY_SUBNETS`])?
    ///
    /// A create asks this so it can refuse with the quota code instead of overshooting the
    /// ceiling; [`Self::insert_secondary_subnet`] asks it again, so a writer that forgets
    /// the gate still cannot grow the store past it.
    pub fn has_secondary_subnet_capacity(&self) -> bool {
        self.secondary_subnets.len() < MAX_STORED_SECONDARY_SUBNETS
    }

    /// Store a secondary subnet, and answer the `SecondarySubnetId` it was given — or
    /// `None` when this store is already at [`MAX_STORED_SECONDARY_SUBNETS`], which a
    /// handler answers with [`secondary_subnet_limit_exceeded`].
    ///
    /// Everything the service itself owns is derived here rather than taken from the
    /// caller — the subnet's id, the ARN built from it, the owning account, and the
    /// association id of each IPv4 CIDR block — so two writers cannot report the same
    /// secondary subnet differently. Like [`Self::insert_subnet`], the caller-chosen bytes
    /// the record holds are charged against the store's byte budget here, so no writer can
    /// store them uncharged, and the ceiling is checked BEFORE any id is minted, so a
    /// refused write consumes nothing at all.
    pub fn insert_secondary_subnet(&mut self, spec: SecondarySubnetSpec) -> Option<String> {
        if !self.has_secondary_subnet_capacity() {
            return None;
        }
        let secondary_subnet_id = self.next_id(SECONDARY_SUBNET_ID_PREFIX);
        let mut ipv4_cidr_block_associations = Vec::with_capacity(spec.ipv4_cidr_blocks.len());
        for block in spec.ipv4_cidr_blocks {
            let association_id = self.next_id(SECONDARY_SUBNET_CIDR_ASSOCIATION_ID_PREFIX);
            ipv4_cidr_block_associations.push(SecondarySubnetIpv4CidrBlockAssociation {
                association_id: Some(association_id),
                cidr_block: Some(block.cidr_block),
                state: Some(block.state),
                state_reason: block.state_reason,
            });
        }
        let secondary_subnet = SecondarySubnet {
            secondary_subnet_id: Some(secondary_subnet_id.clone()),
            secondary_subnet_arn: Some(::engine::emulator_arn(
                "ec2",
                &spec.region,
                &format!("secondary-subnet/{secondary_subnet_id}"),
            )),
            secondary_network_id: Some(spec.secondary_network_id),
            secondary_network_type: Some(spec.secondary_network_type),
            owner_id: Some(::engine::emulator_account_id().to_string()),
            availability_zone_id: Some(spec.availability_zone_id),
            availability_zone: Some(spec.availability_zone),
            ipv4_cidr_block_associations: Some(ipv4_cidr_block_associations),
            state: Some(spec.state),
            state_reason: spec.state_reason,
            tags: Some(spec.tags),
        };
        self.add_stored_bytes(secondary_subnet_payload_bytes(&secondary_subnet));
        self.secondary_subnets
            .insert(secondary_subnet_id.clone(), secondary_subnet);
        Some(secondary_subnet_id)
    }
}

/// How many secondary subnets this emulator stores before a write of another is refused.
///
/// CHOSEN, like [`MAX_SECONDARY_NETWORKS`] and [`MAX_STORED_RECORDS_PER_STORE`] and for
/// the same
/// reason the module gives for every other ceiling: nothing in this emulator reclaims a
/// stored secondary subnet, so this store needs a ceiling that is not the process's memory.
/// It is the same ceiling the other record stores keep.
pub const MAX_STORED_SECONDARY_SUBNETS: usize = MAX_STORED_RECORDS_PER_STORE;

/// Refuse a create that would take the store past [`MAX_STORED_SECONDARY_SUBNETS`]: HTTP
/// 400, with the same quota code family a secondary network's own ceiling answers with.
pub fn secondary_subnet_limit_exceeded<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(SECONDARY_NETWORK_LIMIT_EXCEEDED, 400, message)
}

/// The bytes one stored secondary subnet is charged: its tags, the CIDR block of each
/// association, and the state reasons, plus [`STORED_RECORD_OVERHEAD_BYTES`] for the
/// record itself.
/// The ids, the ARN, the owning account, the states and the network type are derived by
/// this module or bounded by the model, so they are fixed-size.
///
/// The fixed overhead is what makes a subnet written with no tags, no IPv4 CIDR block and
/// no state reason cost something: a count ceiling alone leaves a zero-payload record free
/// against [`MAX_STORED_BYTES`], so repeated writes would advance no budget at all.
///
/// The ONE definition of that charge, the way [`vpc_endpoint_connection_payload_bytes`] is
/// for a VPC endpoint connection: a writer charges exactly it, and a removal path must give
/// back the same number.
pub fn secondary_subnet_payload_bytes(secondary_subnet: &SecondarySubnet) -> usize {
    let association_bytes: usize = secondary_subnet
        .ipv4_cidr_block_associations
        .iter()
        .flatten()
        .map(|association| {
            association.cidr_block.as_deref().unwrap_or_default().len()
                + association.state_reason.as_deref().unwrap_or_default().len()
        })
        .sum();
    tag_list_bytes(secondary_subnet.tags.as_deref().unwrap_or_default())
        .saturating_add(association_bytes)
        .saturating_add(
            secondary_subnet
                .state_reason
                .as_deref()
                .unwrap_or_default()
                .len(),
        )
        .saturating_add(STORED_RECORD_OVERHEAD_BYTES)
}

// ---------------------------------------------------------------------------
// VPC Block Public Access exclusions
// ---------------------------------------------------------------------------

/// The id prefix of a VPC BPA exclusion, as AWS mints one: `vpcbpa-exclude-<hex>`.
///
/// [`ComAmazonawsEc2Amazonec2State::next_id`] appends the 17 lowercase hexadecimal digits
/// every EC2 resource id of the long form carries.
pub const VPC_BPA_EXCLUSION_ID_PREFIX: &str = "vpcbpa-exclude";

/// An exclusion AWS is still putting in place: the `VpcBlockPublicAccessExclusionState`
/// enum's `create-in-progress` member, which is what
/// `CreateVpcBlockPublicAccessExclusion` answers with until [`TRANSITION_DELAY`] has
/// passed.
pub const VPC_BPA_EXCLUSION_STATE_CREATE_IN_PROGRESS: &str = "create-in-progress";

/// An exclusion that is in place: the `VpcBlockPublicAccessExclusionState` enum's
/// `create-complete` member, the state the clock owes a `create-in-progress` one.
pub const VPC_BPA_EXCLUSION_STATE_CREATE_COMPLETE: &str = "create-complete";

/// The `ResourceType` a `CreateVpcBlockPublicAccessExclusion` tag specification may name:
/// the exclusion it creates (the modelled `ResourceType` enum's
/// `vpc-block-public-access-exclusion` member).
pub const RESOURCE_TYPE_VPC_BLOCK_PUBLIC_ACCESS_EXCLUSION: &str =
    "vpc-block-public-access-exclusion";

/// The largest number of VPC BPA exclusions this in-memory store holds.
///
/// A capacity bound in the same style as [`MAX_VPCS`], not an AWS quota: each exclusion is
/// a record in this process's memory, so the number a caller can create has to be finite.
/// [`ComAmazonawsEc2Amazonec2State::has_vpc_bpa_exclusion_capacity`] is the predicate a
/// writer asks before inserting; a create past it refuses with
/// [`vpc_bpa_exclusion_limit_exceeded`].
pub const MAX_VPC_BPA_EXCLUSIONS: usize = 1_000;

/// The code a create past [`MAX_VPC_BPA_EXCLUSIONS`] answers with.
///
/// EC2 names every per-resource quota refusal `<Resource>LimitExceeded`
/// ([`VPC_LIMIT_EXCEEDED`], [`SUBNET_LIMIT_EXCEEDED`], [`SECURITY_GROUP_LIMIT_EXCEEDED`],
/// [`ADDRESS_LIMIT_EXCEEDED`]), and EC2's model declares no error shapes at all, so this
/// resource's code follows that convention.
pub const VPC_BPA_EXCLUSION_LIMIT_EXCEEDED: &str = "VpcBlockPublicAccessExclusionLimitExceeded";

/// The code EC2 answers for an exclusion id the account holds no exclusion under.
///
/// The same `Invalid<Resource>Id.NotFound` convention as
/// [`INVALID_LAUNCH_TEMPLATE_ID_NOT_FOUND`].
pub const INVALID_VPC_BPA_EXCLUSION_ID_NOT_FOUND: &str =
    "InvalidVpcBlockPublicAccessExclusionId.NotFound";

/// What a VPC BPA exclusion is applied to: "a mode that can be applied to a single VPC or
/// subnet" (`VpcBlockPublicAccessExclusion`).
///
/// The modelled shape carries no member for it — the exclusion reports its own ARN, not
/// the excluded resource's id — so it is stored beside the record under the same id, the
/// way [`ComAmazonawsEc2Amazonec2State::vpc_attributes`] holds what a [`Vpc`] has no member
/// for. Which resource an exclusion covers is what makes a second exclusion for the same
/// VPC or subnet a conflict, and what a delete has to know to release it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExclusionTarget {
    /// "A VPC ID" (`CreateVpcBlockPublicAccessExclusionRequest$VpcId`).
    Vpc(String),
    /// "A subnet ID" (`CreateVpcBlockPublicAccessExclusionRequest$SubnetId`).
    Subnet(String),
}

impl ExclusionTarget {
    /// The id of the excluded VPC or subnet.
    pub fn resource_id(&self) -> &str {
        match self {
            Self::Vpc(id) | Self::Subnet(id) => id,
        }
    }

    /// The modelled `ResourceType` word for the excluded resource, so a refusal can name
    /// what it is as well as which.
    pub fn resource_type(&self) -> &'static str {
        match self {
            Self::Vpc(_) => RESOURCE_TYPE_VPC,
            Self::Subnet(_) => RESOURCE_TYPE_SUBNET,
        }
    }
}

/// The move the clock owes one exclusion: a [`ClockTransition`] to the
/// `VpcBlockPublicAccessExclusionState` member it becomes, reported as the exclusion's
/// `LastUpdateTimestamp`.
///
/// The `to` is a `&'static str` so only a word from this module's own catalog constants can
/// be stored — never a request's text.
pub type ExclusionTransition = ClockTransition<&'static str>;

/// One VPC BPA exclusion: the modelled [`VpcBlockPublicAccessExclusion`] every operation
/// that reports one answers with, plus the two facts that shape carries no member for.
///
/// `CreateVpcBlockPublicAccessExclusion`, `DescribeVpcBlockPublicAccessExclusions`,
/// `ModifyVpcBlockPublicAccessExclusion` and `DeleteVpcBlockPublicAccessExclusion` all
/// answer with that one shape, so the one stored view is what all of them report and they
/// cannot describe one exclusion differently.
#[derive(Debug, Clone)]
pub struct VpcBlockPublicAccessExclusionRecord {
    /// The exclusion as the model shapes it, carrying the state reached so far.
    pub view: VpcBlockPublicAccessExclusion,
    /// The VPC or subnet this exclusion is applied to.
    pub target: ExclusionTarget,
    /// The move the clock owes this exclusion, while one is owed.
    pub pending: Option<ExclusionTransition>,
}

impl VpcBlockPublicAccessExclusionRecord {
    /// The exclusion's `VpcBlockPublicAccessExclusionState` as it was last reported.
    pub fn state(&self) -> &str {
        self.view.state.as_deref().unwrap_or_default()
    }

    /// The exclusion's own id.
    pub fn exclusion_id(&self) -> &str {
        self.view.exclusion_id.as_deref().unwrap_or_default()
    }
}

/// What a writer must say to store a VPC BPA exclusion; everything else about the record
/// is derived by [`ComAmazonawsEc2Amazonec2State::insert_vpc_block_public_access_exclusion`].
#[derive(Debug, Clone)]
pub struct VpcBlockPublicAccessExclusionSpec {
    /// The VPC or subnet the exclusion is applied to.
    pub target: ExclusionTarget,
    /// "The exclusion mode for internet gateway traffic"
    /// (`VpcBlockPublicAccessExclusion$InternetGatewayExclusionMode`), as a `&'static str`
    /// out of the modelled catalog ([`internet_gateway_exclusion_mode`]) so the stored
    /// word is never the request's own text.
    pub internet_gateway_exclusion_mode: &'static str,
    /// The Region the request was signed for, which the exclusion's ARN names.
    pub region: String,
    /// The tags the request's `TagSpecifications` assign to the exclusion.
    pub tags: Vec<Tag>,
}

/// The modelled `InternetGatewayExclusionMode` member `requested` names, or `None` when it
/// names none.
///
/// The catalog is the generated [`crate::ops::types::INTERNET_GATEWAY_EXCLUSION_MODE_VALUES`],
/// so this crate holds no second copy of it. Answering a `&'static str` out of that catalog
/// is what keeps request text out of the store.
pub fn internet_gateway_exclusion_mode(requested: &str) -> Option<&'static str> {
    crate::ops::types::INTERNET_GATEWAY_EXCLUSION_MODE_VALUES
        .iter()
        .copied()
        .find(|mode| *mode == requested)
}

/// Refuse a create that would take the store past [`MAX_VPC_BPA_EXCLUSIONS`]: HTTP 400
/// [`VPC_BPA_EXCLUSION_LIMIT_EXCEEDED`].
pub fn vpc_bpa_exclusion_limit_exceeded<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        VPC_BPA_EXCLUSION_LIMIT_EXCEEDED,
        400,
        format!(
            "The maximum number of VPC Block Public Access exclusions has been reached \
             ({MAX_VPC_BPA_EXCLUSIONS})"
        ),
    )
}

/// Refuse an exclusion id a request named that the account holds no exclusion under: HTTP
/// 400 [`INVALID_VPC_BPA_EXCLUSION_ID_NOT_FOUND`].
pub fn invalid_vpc_bpa_exclusion_id_not_found<E>(exclusion_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_VPC_BPA_EXCLUSION_ID_NOT_FOUND,
        400,
        format!("The VPC Block Public Access exclusion ID '{exclusion_id}' does not exist"),
    )
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another VPC BPA exclusion fits inside the store's ceiling
    /// ([`MAX_VPC_BPA_EXCLUSIONS`]).
    ///
    /// The same contract as [`Self::has_vpc_capacity`]: a create asks first and refuses
    /// with [`vpc_bpa_exclusion_limit_exceeded`] when it answers `false`.
    pub fn has_vpc_bpa_exclusion_capacity(&self) -> bool {
        self.vpc_block_public_access_exclusions.len() < MAX_VPC_BPA_EXCLUSIONS
    }

    /// The id of the exclusion already applied to `target`, if the account holds one.
    ///
    /// An exclusion is "a mode that can be applied to a single VPC or subnet"
    /// (`VpcBlockPublicAccessExclusion`), so one resource carries one mode: a second
    /// exclusion for the same VPC or subnet would leave two modes in force on it with no
    /// rule saying which wins. A create asks this first and refuses rather than storing a
    /// conflict.
    pub fn vpc_bpa_exclusion_for_target(&self, target: &ExclusionTarget) -> Option<&str> {
        self.vpc_block_public_access_exclusions
            .values()
            .find(|record| record.target == *target)
            .map(VpcBlockPublicAccessExclusionRecord::exclusion_id)
    }

    /// Store a VPC BPA exclusion and answer it as it was stored.
    ///
    /// The one way an exclusion is stored, so the id it carries, the ARN it reports, the
    /// state it starts in, the move the clock owes it and the bytes it charges cannot be
    /// built two ways. An exclusion is answered in the non-terminal `create-in-progress`
    /// state, so the clock owes it `create-complete` one [`TRANSITION_DELAY`] later.
    ///
    /// What comes back is the stored view itself, so a create answers exactly what a later
    /// `DescribeVpcBlockPublicAccessExclusions` will report without reading the store a
    /// second time.
    pub fn insert_vpc_block_public_access_exclusion(
        &mut self,
        spec: VpcBlockPublicAccessExclusionSpec,
    ) -> VpcBlockPublicAccessExclusion {
        let exclusion_id = self.next_id(VPC_BPA_EXCLUSION_ID_PREFIX);
        // "When the exclusion was created"
        // (`VpcBlockPublicAccessExclusion$CreationTimestamp`); the exclusion has not
        // changed since, so that is also when it was last updated.
        let created_at = system_time_to_date_time(SystemTime::now());
        let view = VpcBlockPublicAccessExclusion {
            exclusion_id: Some(exclusion_id.clone()),
            internet_gateway_exclusion_mode: Some(spec.internet_gateway_exclusion_mode.to_string()),
            // "The ARN of the exclusion" (`VpcBlockPublicAccessExclusion$ResourceArn`),
            // which the `resource-arn` filter of the describe calls "the Amazon Resource
            // Name (ARN) of a exclusion" too: an EC2 ARN in the Region the request was
            // signed for, under the `vpc-block-public-access-exclusion` resource type the
            // modelled `ResourceType` catalog publishes for this resource.
            resource_arn: Some(::engine::emulator_arn(
                "ec2",
                &spec.region,
                &format!("{RESOURCE_TYPE_VPC_BLOCK_PUBLIC_ACCESS_EXCLUSION}/{exclusion_id}"),
            )),
            state: Some(VPC_BPA_EXCLUSION_STATE_CREATE_IN_PROGRESS.to_string()),
            // "The reason for the current exclusion state": nothing has gone wrong with
            // this one, so there is nothing to report.
            reason: None,
            creation_timestamp: Some(created_at),
            last_update_timestamp: Some(created_at),
            // "When the exclusion was deleted": it has not been.
            deletion_timestamp: None,
            tags: Some(spec.tags),
        };
        self.add_stored_bytes(tag_list_bytes(view.tags.as_deref().unwrap_or_default()));
        let stored = view.clone();
        self.vpc_block_public_access_exclusions.insert(
            exclusion_id,
            VpcBlockPublicAccessExclusionRecord {
                view,
                target: spec.target,
                pending: Some(ExclusionTransition::starting_now(
                    VPC_BPA_EXCLUSION_STATE_CREATE_COMPLETE,
                )),
            },
        );
        stored
    }
}

// ---------------------------------------------------------------------------
// Placement groups
// ---------------------------------------------------------------------------

/// The id prefix of a placement group, as AWS mints one: `pg-<hex>`.
///
/// [`ComAmazonawsEc2Amazonec2State::next_id`] appends the 17 lowercase hexadecimal digits
/// every EC2 resource id of the long form carries.
pub const PLACEMENT_GROUP_ID_PREFIX: &str = "pg";

/// A placement group that can be launched into: the `PlacementGroupState` enum's
/// `available` member.
///
/// A placement group is "a logical grouping of instances" (`CreatePlacementGroup`) — there
/// is no hardware to bring up for it — so it is the state a group is created in and the
/// only state this emulator ever reports one in.
pub const PLACEMENT_GROUP_STATE_AVAILABLE: &str = "available";

/// The `ResourceType` a `CreatePlacementGroup` tag specification may name: the placement
/// group it creates (the modelled `ResourceType` enum's `placement-group` member).
pub const RESOURCE_TYPE_PLACEMENT_GROUP: &str = "placement-group";

/// The `PlacementStrategy` member a partition placement group is created with: the one
/// strategy `CreatePlacementGroupRequest$PartitionCount` is "valid only when" set to.
pub const PLACEMENT_STRATEGY_PARTITION: &str = "partition";

/// The `PlacementStrategy` member a spread placement group is created with: the strategy
/// `SpreadLevel` — "determines how placement groups spread instances" — belongs to.
pub const PLACEMENT_STRATEGY_SPREAD: &str = "spread";

/// The `PlacementStrategy` member a cluster placement group is created with: the one
/// strategy `CreatePlacementGroupRequest$ParentGroupId` is "valid only when" set to.
pub const PLACEMENT_STRATEGY_CLUSTER: &str = "cluster";

/// The `SpreadLevel` member a spread placement group gets when the request names none:
/// `rack`, the level with "no usage restrictions", as opposed to `host`, which "you can use
/// only with Outpost placement groups" (`CreatePlacementGroupRequest$SpreadLevel`).
pub const SPREAD_LEVEL_RACK: &str = "rack";

/// "Up to 255 ASCII characters" (`CreatePlacementGroupRequest$GroupName`).
pub const MAX_PLACEMENT_GROUP_NAME_CHARS: usize = 255;

/// The most partitions a partition placement group may be created with.
///
/// "A partition placement group can have a maximum of seven partitions per Availability
/// Zone" (Amazon EC2 User Guide, "Partition placement groups").
pub const MAX_PLACEMENT_GROUP_PARTITION_COUNT: i32 = 7;

/// The fewest partitions a partition placement group may be created with: a count of
/// partitions is a count, so it is at least one.
pub const MIN_PLACEMENT_GROUP_PARTITION_COUNT: i32 = 1;

/// The partition count a partition placement group gets when the request names none.
///
/// AWS documents no default for `CreatePlacementGroupRequest$PartitionCount`, so this is
/// the emulator's choice: the smallest count that actually partitions the group, since a
/// partition group "places groups of instances in different partitions, where instances in
/// one partition do not share the same hardware with instances in another partition"
/// (`CreatePlacementGroup`) and one partition has no other partition to differ from.
pub const DEFAULT_PLACEMENT_GROUP_PARTITION_COUNT: i32 = 2;

/// The largest number of placement groups this store holds.
///
/// "Placement groups per Region: 500" (Amazon EC2 service quotas).
/// [`ComAmazonawsEc2Amazonec2State::has_placement_group_capacity`] is the predicate a
/// writer asks before inserting; a create past it refuses with
/// [`placement_group_limit_exceeded`].
pub const MAX_PLACEMENT_GROUPS: usize = 500;

/// The code EC2 answers for a placement group name the account already holds a group
/// under: "the specified placement group already exists in your account" (AWS's
/// `InvalidPlacementGroup.Duplicate` error-code entry).
pub const INVALID_PLACEMENT_GROUP_DUPLICATE: &str = "InvalidPlacementGroup.Duplicate";

/// The code EC2 answers for a placement group a request named that the account holds
/// none of: "the specified placement group cannot be found" (AWS's
/// `InvalidPlacementGroup.Unknown` error-code entry).
pub const INVALID_PLACEMENT_GROUP_UNKNOWN: &str = "InvalidPlacementGroup.Unknown";

/// The code a create past [`MAX_PLACEMENT_GROUPS`] answers with: "you've reached the limit
/// on the number of placement groups you can have" (AWS's `PlacementGroupLimitExceeded`
/// error-code entry).
pub const PLACEMENT_GROUP_LIMIT_EXCEEDED: &str = "PlacementGroupLimitExceeded";

/// Refuse a second placement group with a name the account already holds: HTTP 400
/// [`INVALID_PLACEMENT_GROUP_DUPLICATE`]. The name "must be unique within the scope of your
/// account for the Region" (`CreatePlacementGroupRequest$GroupName`).
pub fn invalid_placement_group_duplicate<E>(group_name: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_PLACEMENT_GROUP_DUPLICATE,
        400,
        format!("The placement group '{group_name}' already exists"),
    )
}

/// Refuse a placement group a request named that the account holds none of: HTTP 400
/// [`INVALID_PLACEMENT_GROUP_UNKNOWN`]. `group` is the name or the id the request named,
/// whichever could not be resolved.
pub fn invalid_placement_group_unknown<E>(group: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_PLACEMENT_GROUP_UNKNOWN,
        400,
        format!("The placement group '{group}' does not exist"),
    )
}

/// Refuse a create that would take the store past [`MAX_PLACEMENT_GROUPS`]: HTTP 400
/// [`PLACEMENT_GROUP_LIMIT_EXCEEDED`].
pub fn placement_group_limit_exceeded<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        PLACEMENT_GROUP_LIMIT_EXCEEDED,
        400,
        format!("The maximum number of placement groups has been reached ({MAX_PLACEMENT_GROUPS})"),
    )
}

/// The caller-chosen bytes one stored placement group holds, plus the fixed overhead the
/// record itself costs.
///
/// Only the members whose length the REQUEST chose are counted on top of
/// [`STORED_RECORD_OVERHEAD_BYTES`]: the id and the ARN are minted by
/// [`ComAmazonawsEc2Amazonec2State::insert_placement_group`], and the state, strategy and
/// spread level are words out of this module's own catalogs. The ONE definition of that
/// charge, the way [`vpc_endpoint_connection_payload_bytes`] is for a connection: the
/// writer charges exactly it and `CreatePlacementGroup` reserves exactly it, so the budget
/// a create checks cannot drift from what the insert takes, and a group with no tags is not
/// free against [`MAX_STORED_BYTES`].
pub fn placement_group_spec_bytes(spec: &PlacementGroupSpec) -> usize {
    STORED_RECORD_OVERHEAD_BYTES
        .saturating_add(spec.group_name.len())
        .saturating_add(tag_list_bytes(&spec.tags))
        .saturating_add(spec.parent_group_id.as_ref().map_or(0, String::len))
}

/// What a writer must say to store a placement group; everything else about the record —
/// the id, the ARN, the state — is derived by
/// [`ComAmazonawsEc2Amazonec2State::insert_placement_group`].
#[derive(Debug, Clone)]
pub struct PlacementGroupSpec {
    /// "A name for the placement group" (`CreatePlacementGroupRequest$GroupName`).
    pub group_name: String,
    /// "The placement strategy" (`CreatePlacementGroupRequest$Strategy`), as a
    /// `&'static str` out of the modelled `PlacementStrategy` catalog
    /// ([`placement_strategy`]), so the stored word is never the request's own text.
    pub strategy: &'static str,
    /// "The number of partitions" a partition group was created with, and `None` for every
    /// other strategy: `PartitionCount` is "valid only when Strategy is set to partition"
    /// (`CreatePlacementGroupRequest$PartitionCount`).
    pub partition_count: Option<i32>,
    /// "The spread level for the placement group" a spread group was created with, and
    /// `None` for every other strategy, out of the modelled `SpreadLevel` catalog
    /// ([`spread_level`]).
    pub spread_level: Option<&'static str>,
    /// "The ID of the parent placement group" (`PlacementGroup$ParentGroupId`), resolved
    /// against this store, when the request named one.
    pub parent_group_id: Option<String>,
    /// The Region the request was signed for, which the group's ARN names.
    pub region: String,
    /// The tags the request's `TagSpecifications` assign to the group.
    pub tags: Vec<Tag>,
}

/// The modelled `PlacementStrategy` member `requested` names, or `None` when it names
/// none.
///
/// The catalog is the generated [`crate::ops::types::PLACEMENT_STRATEGY_VALUES`], so this
/// crate holds no second copy of it. Answering a `&'static str` out of that catalog is what
/// keeps request text out of the store.
pub fn placement_strategy(requested: &str) -> Option<&'static str> {
    crate::ops::types::PLACEMENT_STRATEGY_VALUES
        .iter()
        .copied()
        .find(|strategy| *strategy == requested)
}

/// The modelled `SpreadLevel` member `requested` names, or `None` when it names none.
///
/// The catalog is the generated [`crate::ops::types::SPREAD_LEVEL_VALUES`], for the reason
/// [`placement_strategy`] gives.
pub fn spread_level(requested: &str) -> Option<&'static str> {
    crate::ops::types::SPREAD_LEVEL_VALUES
        .iter()
        .copied()
        .find(|level| *level == requested)
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another placement group fits inside the store's ceiling
    /// ([`MAX_PLACEMENT_GROUPS`]).
    ///
    /// The same contract as [`Self::has_vpc_capacity`]: a create asks first and refuses
    /// with [`placement_group_limit_exceeded`] when it answers `false`.
    pub fn has_placement_group_capacity(&self) -> bool {
        self.placement_groups.len() < MAX_PLACEMENT_GROUPS
    }

    /// The id of the placement group named `group_name`, if the account holds one.
    ///
    /// A name "must be unique within the scope of your account for the Region"
    /// (`CreatePlacementGroupRequest$GroupName`), so this answers at most one group: it is
    /// both what a create asks before storing a duplicate and how a request that names a
    /// group by name (`DeletePlacementGroup`, `DescribePlacementGroups$GroupNames`)
    /// resolves it. Compared byte for byte, the name being an opaque identifier the model
    /// publishes no case rule for.
    pub fn placement_group_id_for_name(&self, group_name: &str) -> Option<String> {
        self.placement_groups
            .iter()
            .find(|(_, group)| group.group_name.as_deref() == Some(group_name))
            .map(|(group_id, _)| group_id.clone())
    }

    /// Store a placement group and answer it as it was stored — or `None` when the store is
    /// at [`MAX_PLACEMENT_GROUPS`] and nothing was stored.
    ///
    /// The one way a placement group is stored, so the id it carries, the ARN it reports,
    /// the state it starts in and the bytes it charges cannot be built two ways, and the
    /// ceiling is asked HERE as well as by the caller — the contract every writer of this
    /// module follows, so a writer that forgets the gate cannot grow the store without
    /// bound. What comes back is the stored view itself, so a create answers exactly what a
    /// later `DescribePlacementGroups` reports without reading the store a second time.
    pub fn insert_placement_group(
        &mut self,
        spec: PlacementGroupSpec,
    ) -> Option<PlacementGroup> {
        if !self.has_placement_group_capacity() {
            return None;
        }
        let group_id = self.next_id(PLACEMENT_GROUP_ID_PREFIX);
        // Charged from the spec, so the quantity is exactly the one `CreatePlacementGroup`
        // reserved against the byte budget before calling this.
        self.add_stored_bytes(placement_group_spec_bytes(&spec));
        let group = PlacementGroup {
            group_id: Some(group_id.clone()),
            group_name: Some(spec.group_name.clone()),
            // "A logical grouping of instances" has nothing to provision, so it is
            // available as soon as it exists.
            state: Some(PLACEMENT_GROUP_STATE_AVAILABLE.to_string()),
            strategy: Some(spec.strategy.to_string()),
            partition_count: spec.partition_count,
            spread_level: spec.spread_level.map(str::to_string),
            // "The Amazon Resource Name (ARN) of the placement group"
            // (`PlacementGroup$GroupArn`): an EC2 ARN in the Region the request was signed
            // for, under the `placement-group` resource type, naming the group by NAME —
            // the form `PlacementGroupArn`'s own pattern publishes
            // (`arn:aws:ec2:<region>:<account>:placement-group/<name>`).
            group_arn: Some(::engine::emulator_arn(
                "ec2",
                &spec.region,
                &format!("{RESOURCE_TYPE_PLACEMENT_GROUP}/{group_name}", group_name = spec.group_name),
            )),
            // "Reserved for future use" (`PlacementGroup$LinkedGroupId`), and "the service
            // provider that manages the Placement Group" (`PlacementGroup$Operator`), whose
            // request member is "reserved for internal use": AWS publishes no rule by which
            // a customer request gives a group either, and no service provider manages a
            // group an account created for itself, so this emulator reports neither rather
            // than inventing a meaning for them.
            linked_group_id: None,
            operator: None,
            parent_group_id: spec.parent_group_id,
            tags: Some(spec.tags),
        };
        let stored = group.clone();
        self.placement_groups.insert(group_id, group);
        Some(stored)
    }
}

// ---------------------------------------------------------------------------
// Allowed AMIs: the account's image-criteria setting in one Region
// ---------------------------------------------------------------------------
//
// See the module docs for why this is a SETTING keyed by Region rather than a resource,
// and why one record holds both the state and the criteria. The words below are members
// of the model's own enums, named here so the four operations that read or write the
// setting cannot spell one value two ways.

/// The Allowed AMIs state of an account that has not enabled the feature, and the one
/// `DisableAllowedImagesSettings` moves it to: the `AllowedImagesSettingsDisabledState`
/// enum's only member, `disabled` — "All AMIs are allowed"
/// (`GetAllowedImagesSettingsResult$State`). The generated catalog is
/// `crate::ops::types::ALLOWED_IMAGES_SETTINGS_DISABLED_STATE_VALUES`.
pub const ALLOWED_IMAGES_SETTINGS_DISABLED: &str = "disabled";

/// `AllowedImagesSettingsEnabledState`'s `enabled` member — "Only AMIs matching the image
/// criteria are discoverable and available for use". The generated catalog of the two
/// states an enable accepts is
/// `crate::ops::types::ALLOWED_IMAGES_SETTINGS_ENABLED_STATE_VALUES`.
pub const ALLOWED_IMAGES_SETTINGS_ENABLED: &str = "enabled";

/// `AllowedImagesSettingsEnabledState`'s `audit-mode` member — "All AMIs are allowed, but
/// the ImageAllowed field is set to true if the AMI would be allowed with the current list
/// of criteria if allowed AMIs was enabled" (see [`ALLOWED_IMAGES_SETTINGS_ENABLED`] for
/// the catalog).
pub const ALLOWED_IMAGES_SETTINGS_AUDIT_MODE: &str = "audit-mode";

// The entity that manages the setting is `ManagedBy`'s `account` member for Allowed AMIs
// exactly as it is for snapshot block public access — "the Allowed AMIs settings is managed
// by the account" (`GetAllowedImagesSettingsResult$ManagedBy`), the account being the only
// writer of either setting this emulator has — so this crate spells it once, as
// [`MANAGED_BY_ACCOUNT`] above, and `GetAllowedImagesSettings` reports that constant.

/// The most `ImageCriterion` entries one Allowed AMIs setting may carry.
///
/// `ImageCriterionRequest`'s own doc: "The `ImageCriteria` can include up to: 10
/// `ImageCriterion`". The model declares no `@length` on `ImageCriterionRequestList`, so
/// the handler is the only place the bound can be enforced.
pub const MAX_IMAGE_CRITERIA: usize = 10;

/// The most `ImageProviders` values one `ImageCriterion` may carry: "Maximum: 200 values"
/// (`ImageCriterionRequest$ImageProviders`).
pub const MAX_IMAGE_PROVIDERS_PER_CRITERION: usize = 200;

/// The most `ImageNames` values one `ImageCriterion` may carry: "Maximum: 50 values"
/// (`ImageCriterionRequest$ImageNames`).
pub const MAX_IMAGE_NAMES_PER_CRITERION: usize = 50;

/// The most `MarketplaceProductCodes` values one `ImageCriterion` may carry: "Maximum: 50
/// values" (`ImageCriterionRequest$MarketplaceProductCodes`).
pub const MAX_MARKETPLACE_PRODUCT_CODES_PER_CRITERION: usize = 50;

/// The most `ImageWatermarks` entries one `ImageCriterion` may carry: "50 values for
/// `ImageWatermarks`" (`ImageCriterionRequest`'s own doc).
pub const MAX_IMAGE_WATERMARKS_PER_CRITERION: usize = 50;

/// The image-provider words `ImageProviders` accepts beside an account id and
/// [`IMAGE_PROVIDER_NONE`]: "`amazon`: Allow AMIs created by Amazon or verified providers.
/// `aws-marketplace`: Allow AMIs created by verified providers in the Amazon Web Services
/// Marketplace. `aws-backup-vault`: Allow AMIs created by Amazon Web Services Backup"
/// (`ImageCriterionRequest$ImageProviders`).
///
/// The same three aliases `DescribeImages`'s `Owners` accepts ([`IMAGE_OWNER_ALIASES`]),
/// which is why that catalog is the one read here rather than a second copy.
pub const IMAGE_PROVIDER_ALIASES: &[&str] = &IMAGE_OWNER_ALIASES;

/// The `ImageProviders` word that stands for the account itself: "`none`: Allow AMIs
/// created by your own account only. When `none` is specified, no other values can be
/// specified" (`ImageCriterionRequest$ImageProviders`).
pub const IMAGE_PROVIDER_NONE: &str = "none";

/// The fewest characters an `ImageNames` value may carry: "Length: 1–128 characters"
/// (`ImageCriterionRequest$ImageNames`).
pub const MIN_IMAGE_NAME_CRITERION_CHARS: usize = 1;

/// The most characters an `ImageNames` value may carry: the ceiling of that same
/// documented "Length: 1–128 characters".
///
/// Enforced by this crate rather than relied on from the model: the `@length` the model
/// carries sits on `ImageNameCriteriaRequestList`'s member, and a handler must not depend on
/// where the request walk happens to apply it — without a per-STRING ceiling every stored
/// name would be caller-sized.
pub const MAX_IMAGE_NAME_CRITERION_CHARS: usize = 128;

/// The fewest characters an `ImageNames` value carrying a `?` wildcard may have: "With
/// `?`, the minimum is 3 characters" (`ImageCriterionRequest$ImageNames`).
pub const MIN_IMAGE_NAME_CRITERION_CHARS_WITH_WILDCARD: usize = 3;

/// The special characters an `ImageNames` value may carry beside letters, numbers and
/// spaces: "Special characters: `( ) [ ] . / - ' @ _ * ?`"
/// (`ImageCriterionRequest$ImageNames`).
pub const IMAGE_NAME_CRITERION_SPECIAL_CHARACTERS: &str = "()[]./-'@_*?";

/// The fewest characters a `MarketplaceProductCodes` value may carry: "Length: 1-25
/// characters" (`ImageCriterionRequest$MarketplaceProductCodes`).
pub const MIN_MARKETPLACE_PRODUCT_CODE_CHARS: usize = 1;

/// The most characters a `MarketplaceProductCodes` value may carry (see
/// [`MIN_MARKETPLACE_PRODUCT_CODE_CHARS`]); the model declares no `@length` for it.
pub const MAX_MARKETPLACE_PRODUCT_CODE_CHARS: usize = 25;

/// The most characters one `ImageWatermarkFilter` pattern — a `WatermarkKey` or a
/// `SourceImageRegion` — may carry.
///
/// CHOSEN, not an AWS-documented length: the model bounds neither, and both are
/// caller-chosen strings this store keeps. A watermark key is an `accountId:name` and a
/// source image region is a Region name, with `*` and `?` wildcards allowed in either, so
/// this is far above anything either form needs while keeping one stored setting's size
/// finite.
pub const MAX_IMAGE_WATERMARK_PATTERN_CHARS: usize = 128;

/// The most Regions this emulator holds an Allowed AMIs setting for.
///
/// [`MAX_STORED_RECORDS_PER_STORE`] is the crate's ceiling for a store nothing reclaims, and
/// this store is bounded by it like every other: a setting is keyed by the Region of the
/// request's own SigV4 credential scope, so without a ceiling a caller could write a setting
/// for unboundedly many Region names. The bytes each setting holds are charged against
/// [`MAX_STORED_BYTES`] on top of that ([`image_criteria_payload_bytes`]), so neither the
/// number of settings nor their size is unbounded.
pub const MAX_ALLOWED_IMAGES_SETTINGS_REGIONS: usize = MAX_STORED_RECORDS_PER_STORE;

/// The account's Allowed AMIs setting in ONE Region: the state the feature is in, and the
/// criteria that are evaluated when it is applied.
///
/// Held in [`ComAmazonawsEc2Amazonec2State::allowed_images_settings`] under the Region's
/// name. The criteria are the modelled [`ImageCriterion`] shape `GetAllowedImagesSettings`
/// reports, so a replace converts and validates the request shapes once rather than at
/// every read.
#[derive(Debug, Clone)]
pub struct AllowedImagesSettings {
    /// "The current state of the Allowed AMIs setting at the account level in the specified
    /// Amazon Web Services Region" (`GetAllowedImagesSettingsResult$State`): one of
    /// [`ALLOWED_IMAGES_SETTINGS_DISABLED`], [`ALLOWED_IMAGES_SETTINGS_ENABLED`] or
    /// [`ALLOWED_IMAGES_SETTINGS_AUDIT_MODE`].
    pub state: String,
    /// "The list of criteria for images that are discoverable and usable in the account in
    /// the specified Amazon Web Services Region"
    /// (`GetAllowedImagesSettingsResult$ImageCriteria`), at most
    /// [`MAX_IMAGE_CRITERIA`] of them.
    pub image_criteria: Vec<ImageCriterion>,
}

/// The setting of a Region nobody has configured: `disabled` — "All AMIs are allowed" — with
/// no criteria, which is what an account that has never enabled Allowed AMIs has.
impl Default for AllowedImagesSettings {
    fn default() -> Self {
        Self {
            state: ALLOWED_IMAGES_SETTINGS_DISABLED.to_string(),
            image_criteria: Vec::new(),
        }
    }
}

impl ComAmazonawsEc2Amazonec2State {
    /// The Allowed AMIs setting of `region` as it stands.
    ///
    /// A READ: a Region with no stored setting answers [`AllowedImagesSettings::default`]
    /// without storing it, so `GetAllowedImagesSettings` cannot change what a later caller
    /// sees.
    pub fn allowed_images_settings_in(&self, region: &str) -> Cow<'_, AllowedImagesSettings> {
        match self.allowed_images_settings.get(region) {
            Some(settings) => Cow::Borrowed(settings),
            None => Cow::Owned(AllowedImagesSettings::default()),
        }
    }

    /// Move the state of `region`'s Allowed AMIs setting to `state`, leaving its criteria as
    /// they stand — what `EnableAllowedImagesSettings` and `DisableAllowedImagesSettings`
    /// do.
    ///
    /// `Err` means NOTHING was stored and the writer must refuse the request; see
    /// [`AllowedImagesSettingsFull`].
    pub fn set_allowed_images_settings_state(
        &mut self,
        region: &str,
        state: &str,
    ) -> Result<(), AllowedImagesSettingsFull> {
        self.allowed_images_settings_entry(region)?.state = state.to_string();
        Ok(())
    }

    /// Replace the image criteria of `region`'s Allowed AMIs setting, leaving its state as it
    /// stands — what `ReplaceImageCriteriaInAllowedImagesSettings` does.
    ///
    /// The bytes the new criteria hold are charged against [`MAX_STORED_BYTES`] and the ones
    /// the criteria they replace held are given back, so a store that is replaced over and
    /// over does not drift, and a replace that would cross the budget is refused with
    /// NOTHING stored ([`AllowedImagesSettingsFull::Bytes`]).
    pub fn set_allowed_images_criteria(
        &mut self,
        region: &str,
        criteria: Vec<ImageCriterion>,
    ) -> Result<(), AllowedImagesSettingsFull> {
        let requested = image_criteria_payload_bytes(&criteria);
        let released = self
            .allowed_images_settings
            .get(region)
            .map_or(0, |settings| {
                image_criteria_payload_bytes(&settings.image_criteria)
            });
        // Only the GROWTH has to fit: a replace gives the bytes of the criteria it replaces
        // back, so the charge is the difference.
        if self.would_exceed_stored_bytes(requested.saturating_sub(released)) {
            return Err(AllowedImagesSettingsFull::Bytes { requested });
        }
        self.allowed_images_settings_entry(region)?.image_criteria = criteria;
        self.release_stored_bytes(released);
        self.add_stored_bytes(requested);
        Ok(())
    }

    /// The Allowed AMIs setting of `region` to write to, started at
    /// [`AllowedImagesSettings::default`] when the Region has none yet and charged
    /// [`STORED_RECORD_OVERHEAD_BYTES`] for the record it adds, the way every other store's
    /// writer charges for one.
    ///
    /// `Err` once the store holds settings for [`MAX_ALLOWED_IMAGES_SETTINGS_REGIONS`] other
    /// Regions: a writer refuses rather than growing past the ceiling. Private, so the two
    /// public writers above are the only way a setting is written and neither can skip the
    /// ceiling or the charge.
    fn allowed_images_settings_entry(
        &mut self,
        region: &str,
    ) -> Result<&mut AllowedImagesSettings, AllowedImagesSettingsFull> {
        if !self.allowed_images_settings.contains_key(region) {
            if self.allowed_images_settings.len() >= MAX_ALLOWED_IMAGES_SETTINGS_REGIONS {
                return Err(AllowedImagesSettingsFull::Regions);
            }
            self.add_stored_bytes(STORED_RECORD_OVERHEAD_BYTES);
        }
        Ok(self
            .allowed_images_settings
            .entry(region.to_string())
            .or_default())
    }
}

/// Why a write to an Allowed AMIs setting was refused, with NOTHING stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllowedImagesSettingsFull {
    /// The store already holds the settings of
    /// [`MAX_ALLOWED_IMAGES_SETTINGS_REGIONS`] other Regions.
    Regions,
    /// The criteria would take the store past [`MAX_STORED_BYTES`]; `requested` is what they
    /// would have cost.
    Bytes {
        /// The bytes the refused criteria would have held.
        requested: usize,
    },
}

/// The caller-chosen bytes one Allowed AMIs criteria list holds: its image providers, image
/// names, Marketplace product codes and watermark patterns. The day conditions are two
/// integers bounded by the model's own range, so they are fixed-size.
///
/// The ONE definition of that charge, the way [`vpc_endpoint_connection_payload_bytes`] is
/// for a connection: a writer charges exactly it, and a replace gives back exactly what the
/// criteria it replaces cost.
pub fn image_criteria_payload_bytes(criteria: &[ImageCriterion]) -> usize {
    let text_bytes = |values: Option<&Vec<String>>| -> usize {
        values.into_iter().flatten().map(String::len).sum()
    };
    criteria
        .iter()
        .map(|criterion| {
            let watermark_bytes: usize = criterion
                .image_watermarks
                .iter()
                .flatten()
                .map(|watermark| {
                    watermark.watermark_key.as_deref().unwrap_or_default().len()
                        + watermark
                            .source_image_region
                            .as_deref()
                            .unwrap_or_default()
                            .len()
                })
                .sum();
            text_bytes(criterion.image_providers.as_ref())
                .saturating_add(text_bytes(criterion.image_names.as_ref()))
                .saturating_add(text_bytes(criterion.marketplace_product_codes.as_ref()))
                .saturating_add(watermark_bytes)
        })
        .fold(0usize, usize::saturating_add)
}

/// Refuse a write to an Allowed AMIs setting this emulator has no room for: HTTP 400, EC2's
/// `InvalidParameterValue`.
///
/// A bad request rather than a server error: nothing is wrong with the emulator, the Region
/// and the criteria are values of the request, and a retry of the same request would be
/// refused the same way.
pub fn allowed_images_settings_full<E>(region: &str, full: AllowedImagesSettingsFull) -> Scaffolded<E> {
    match full {
        AllowedImagesSettingsFull::Regions => invalid_parameter_value(format!(
            "This emulator holds the Allowed AMIs settings of \
             {MAX_ALLOWED_IMAGES_SETTINGS_REGIONS} Regions, which is as many as it stores, so \
             it cannot store the settings of '{region}' too"
        )),
        AllowedImagesSettingsFull::Bytes { requested } => invalid_parameter_value(format!(
            "The image criteria carry {requested} bytes, which this emulator has no room for: \
             it stores at most {MAX_STORED_BYTES} bytes of caller-supplied data"
        )),
    }
}
