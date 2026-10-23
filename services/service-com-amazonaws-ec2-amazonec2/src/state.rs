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
//! connection is stored, so the id it carries and the bytes it charges cannot be minted two
//! ways. A connection sits in `PendingAcceptance` until the SERVICE OWNER accepts or
//! rejects it (`AcceptVpcEndpointConnections` / `RejectVpcEndpointConnections`), which is
//! an action rather than an instant, so — unlike a `pending` VPC — nothing beside it
//! records a clock-owed transition.
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

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::Ipv6Addr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    Address, ApplicationStatus, ApplicationStatusDetail, AvailabilityZone,
    AvailabilityZoneMessage, BlockDeviceMapping, BlockPublicAccessStates, CapacityBlock,
    CreateVolumeOutput, DescribeFastLaunchImagesSuccessItem, DisableFastLaunchOutput,
    EbsBlockDevice, EnableFastLaunchOutput, GroupIdentifier, Host, INSTANCE_TYPE_VALUES, Image,
    Instance, InstanceApplicationStatus,
    InstanceState, InstanceTypeOffering, IpPermission, IpRange, LOCATION_TYPE_VALUES,
    LaunchTemplate, MacHost, Monitoring, OperatorResponse, Placement,
    PrivateDnsNameOptionsOnLaunch,
    DnsEntry, PayerResponsibilityEntry,
    RequestLaunchTemplateData, RunInstancesOutput, SecurityGroup, StateReason, Subnet, Tag,
    TagSpecification,
    UserIdGroupPair, Vpc, VpcCidrBlockAssociation, VpcCidrBlockState, VpcEndpointConnection,
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
            images: BTreeMap::new(),
            security_groups: BTreeMap::new(),
            launch_templates: BTreeMap::new(),
            launch_template_client_tokens: HashMap::new(),
            id_counter: 0,
            volumes: BTreeMap::new(),
            volume_client_tokens: HashMap::new(),
            capacity_blocks: BTreeMap::new(),
            application_status: BTreeMap::new(),
            fast_launch_images: BTreeMap::new(),
            addresses: BTreeMap::new(),
            next_elastic_ip: 0,
            dedicated_hosts: BTreeMap::new(),
            dedicated_host_client_tokens: HashMap::new(),
            vpc_endpoint_connections: BTreeMap::new(),
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
    /// has one — an instance's next state, a `pending` VPC becoming `available`
    /// with its IPv6 association moving `associating` -> `associated`, a `pending`
    /// subnet becoming `available`, and a Windows fast launch configuration reaching
    /// `enabled` or finishing the cleanup a disable started.
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

/// Whether `id` is a well-formed EC2 instance id.
///
/// An instance id is `i-` followed by 8 or 17 lowercase hexadecimal digits — the
/// short and the long id form. EC2 answers `InvalidInstanceID.Malformed` for
/// anything else, BEFORE it looks the id up, which is a different answer from
/// `InvalidInstanceID.NotFound` for a well-formed id nothing was launched for.
pub fn instance_id_is_well_formed(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix("i-") else {
        return false;
    };
    matches!(suffix.len(), 8 | 17)
        && suffix
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
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
}

impl FilterPattern {
    /// Compile one filter value.
    pub fn compile(pattern: &str) -> Self {
        let mut tokens: Vec<FilterToken> = Vec::new();
        let mut chars = pattern.chars();
        while let Some(c) = chars.next() {
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
        Self { tokens }
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
    pub fn matches_within(&self, value: &str, budget: &mut MatchBudget) -> bool {
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
        // The value is exhausted: what is left of the pattern must match nothing.
        tokens[token.min(tokens.len())..]
            .iter()
            .all(|token| *token == FilterToken::Many)
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

/// Refuse a `NextToken` this service did not issue: HTTP 400 `InvalidPaginationToken`.
/// The token itself is not echoed — it carried nothing the caller can act on.
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
}

impl VolumeRecord {
    /// This volume as of `now`: the stored view with the `State` the clock has
    /// reached. A read observes the transition the clock already owed without
    /// writing anything.
    pub fn view_at(&self, now: Instant) -> VolumeView {
        let mut view = self.view.clone();
        view.state = Some(self.state_at(now).to_string());
        view
    }

    /// The volume's `VolumeState` at `now`.
    pub fn state_at(&self, now: Instant) -> &'static str {
        if now < self.available_at {
            VOLUME_STATE_CREATING
        } else {
            VOLUME_STATE_AVAILABLE
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
pub fn volume_id_is_well_formed(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix("vol-") else {
        return false;
    };
    matches!(suffix.len(), 8 | 17)
        && suffix
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Refuse a volume id the request named that this service cannot resolve: a malformed
/// one before an unknown one, the way the service orders the two.
pub fn require_known_volume<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    volume_id: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    if !volume_id_is_well_formed(volume_id) {
        return Err(crate::scaffolded::Scaffolded::service_error(
            INVALID_VOLUME_ID_MALFORMED,
            400,
            format!("Invalid id: '{volume_id}'"),
        ));
    }
    if !store.volumes.contains_key(volume_id) {
        return Err(invalid_volume_id_not_found(volume_id));
    }
    Ok(())
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
    let Some(suffix) = id.strip_prefix("snap-") else {
        return false;
    };
    matches!(suffix.len(), 8 | 17)
        && suffix
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
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
    let Some(suffix) = id.strip_prefix("ami-") else {
        return false;
    };
    matches!(suffix.len(), 8 | 17)
        && suffix
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
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
            // (`Image$ImageAllowed`). This account declares no Allowed AMIs criteria, so
            // every image it can see satisfies them.
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

/// `value` as an IPv4 CIDR block with its host bits cleared, with NO rule on the prefix
/// length: the one reader both [`canonical_vpc_ipv4_cidr`] and
/// [`canonical_subnet_ipv4_cidr`] put their own published block size on top of.
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

/// A move the clock owes one configuration, with the instants it falls due on.
#[derive(Debug, Clone)]
pub struct FastLaunchTransition {
    /// What the configuration becomes.
    pub to: FastLaunchMove,
    /// The instant from which the move is due. Monotonic ([`Instant`]), because it is only
    /// ever compared against the clock and never reported, so a wall-clock adjustment
    /// cannot move a configuration back to `enabling`.
    pub due_at: Instant,
    /// The wall-clock instant the move falls due, which the configuration reports as its
    /// `StateTransitionTime` once the move has been applied — the state changed when the
    /// move fell due, not when somebody next asked about it.
    pub at: SystemTime,
}

impl FastLaunchTransition {
    /// A move to `to`, due [`TRANSITION_DELAY`] from now: the span this crate gives every
    /// clock-owed transition, recorded rather than slept for.
    pub fn starting_now(to: FastLaunchMove) -> Self {
        Self {
            to,
            due_at: Instant::now() + TRANSITION_DELAY,
            at: SystemTime::now() + TRANSITION_DELAY,
        }
    }
}

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

    /// Store a newly allocated Elastic IP address, charging the caller-chosen bytes it
    /// holds against the store's byte budget so no path can store them uncharged.
    pub fn insert_address(&mut self, address: Address) -> String {
        let allocation_id = address.allocation_id.clone().unwrap_or_default();
        self.add_stored_bytes(address_payload_bytes(&address));
        self.addresses.insert(allocation_id.clone(), address);
        allocation_id
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

impl ComAmazonawsEc2Amazonec2State {
    /// Store a connection to one of the account's own VPC endpoint services, and answer
    /// the `VpcEndpointConnectionId` it was given.
    ///
    /// The connection id is minted here rather than taken from `spec`, the way
    /// [`Self::insert_subnet`] mints a `SubnetId`, and the record is keyed by its
    /// `VpcEndpointId` because one endpoint connects to exactly one service. The
    /// caller-chosen bytes it holds are charged against the store's byte budget here, so no
    /// path can store them uncharged.
    pub fn insert_vpc_endpoint_connection(&mut self, spec: VpcEndpointConnectionSpec) -> String {
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
        vpc_endpoint_connection_id
    }
}

/// The caller-chosen bytes one stored VPC endpoint connection holds: its tags, the load
/// balancer ARNs of the service it connects to, and its DNS entries. The ids, the state,
/// the timestamp and the Region are derived by this module or bounded by the model, so they
/// are fixed-size.
///
/// The ONE definition of that charge, the way [`address_payload_bytes`] is for an address:
/// a writer charges exactly it, and a removal path must give back the same number.
pub fn vpc_endpoint_connection_payload_bytes(connection: &VpcEndpointConnection) -> usize {
    let arn_bytes = |arns: Option<&Vec<String>>| -> usize {
        arns.into_iter().flatten().map(String::len).sum()
    };
    let dns_bytes: usize = connection
        .dns_entries
        .iter()
        .flatten()
        .map(|entry| {
            entry.dns_name.as_deref().unwrap_or_default().len()
                + entry.hosted_zone_id.as_deref().unwrap_or_default().len()
        })
        .sum();
    tag_list_bytes(connection.tags.as_deref().unwrap_or_default())
        + arn_bytes(connection.network_load_balancer_arns.as_ref())
        + arn_bytes(connection.gateway_load_balancer_arns.as_ref())
        + dns_bytes
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

