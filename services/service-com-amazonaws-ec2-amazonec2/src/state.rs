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
//! way, keyed by `SubnetId`. Further caller-created resources (security groups, …)
//! belong beside them as they are implemented, and should be validated against
//! [`availability_zones_in_region`] rather than against a second zone list.
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

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    AvailabilityZone, AvailabilityZoneMessage, BlockPublicAccessStates, GroupIdentifier,
    INSTANCE_TYPE_VALUES, Instance, InstanceState, InstanceTypeOffering, LOCATION_TYPE_VALUES,
    Monitoring, Placement, PrivateDnsNameOptionsOnLaunch, RunInstancesOutput, StateReason, Subnet,
    Tag, Vpc, VpcCidrBlockAssociation, VpcCidrBlockState,
};

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

/// EC2's error code for a `DryRun` request whose caller holds the permissions the
/// action needs: "If you have the required permissions, the error response is
/// `DryRunOperation`" (the `DryRun` member's own doc).
pub const DRY_RUN_OPERATION: &str = "DryRunOperation";

/// EC2's error code for a request that named a VPC id no VPC of the account has.
pub const INVALID_VPC_ID_NOT_FOUND: &str = "InvalidVpcID.NotFound";

/// EC2's error code for a request that named a subnet id no subnet of the account has.
pub const INVALID_SUBNET_ID_NOT_FOUND: &str = "InvalidSubnetID.NotFound";

/// EC2's error code for a pagination token this service did not issue.
pub const INVALID_PAGINATION_TOKEN: &str = "InvalidPaginationToken";

/// EC2's error code for a failure of the emulator itself, never for anything a request
/// carries: a 5xx is retryable to an SDK's retry classifier.
pub const INTERNAL_ERROR: &str = "InternalError";

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
    /// Every subnet the account holds, by `SubnetId`, as the modelled [`Subnet`] shape
    /// itself — so a member a writer sets round-trips through `DescribeSubnets` without a
    /// field-by-field copy. Ordered by id, which is the order the describe reports and
    /// pages over.
    pub subnets: BTreeMap<String, Subnet>,
    /// Monotonic source of generated resource-id suffixes, so an id is unique within a
    /// run and deterministic across runs.
    pub id_counter: u64,
}

/// A fresh account's EC2 state: the default VPC AWS creates for it, and nothing else.
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
            subnets: BTreeMap::new(),
            id_counter: 0,
        };
        let default_vpc = state.insert_vpc(
            DEFAULT_VPC_CIDR_BLOCK,
            true,
            DEFAULT_INSTANCE_TENANCY,
            VPC_STATE_AVAILABLE,
            Vec::new(),
        );
        state.insert_default_subnets(&default_vpc, DEFAULT_VPC_CIDR_BLOCK, DEFAULT_REGION);
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

    /// Insert a VPC with `cidr_block` as its primary IPv4 CIDR block, in `vpc_state`, and
    /// answer the `VpcId` it was given.
    ///
    /// Every id the record carries — the VPC's own, its primary CIDR association's, and
    /// the DHCP options set associated with it — is minted here, so a VPC never shares an
    /// id with another resource of this store.
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
        self.vpcs.insert(vpc_id.clone(), vpc);
        vpc_id
    }

    /// Insert a subnet, and answer the `SubnetId` it was given.
    ///
    /// Everything the service itself owns is derived here rather than taken from the
    /// caller: the id, the ARN built from it, the owning account, and
    /// `AvailableIpAddressCount` computed from the CIDR block — so two writers cannot
    /// report the same subnet differently.
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
    /// block outside its VPC.
    pub fn insert_default_subnets(
        &mut self,
        vpc_id: &str,
        vpc_cidr_block: &str,
        region: &str,
    ) -> Vec<String> {
        let mut inserted = Vec::new();
        for (index, zone) in availability_zones_in_region(region).into_iter().enumerate() {
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
    /// a later caller would otherwise have seen.
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
