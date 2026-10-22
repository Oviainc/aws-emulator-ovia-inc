// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ec2-amazonec2 service.
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
//! at. Further caller-created resources (subnets, security groups, …) belong beside it
//! as they are implemented, and should be validated against
//! [`availability_zones_in_region`] rather than against a second zone list.

use std::collections::BTreeMap;

use crate::ops::types::{
    AvailabilityZone, AvailabilityZoneMessage, BlockPublicAccessStates, INSTANCE_TYPE_VALUES,
    InstanceTypeOffering, LOCATION_TYPE_VALUES, Tag, Vpc, VpcCidrBlockAssociation,
    VpcCidrBlockState,
};

/// The Region a request belongs to when it carries no SigV4 credential scope to read
/// one from — `::engine::emulator_arn`'s documented default Region.
pub const DEFAULT_REGION: &str = "us-east-1";

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

/// VPC Block Public Access turned off — "`off`: VPC BPA is not enabled and traffic is
/// allowed to and from internet gateways and egress-only internet gateways in this
/// Region" (`BlockPublicAccessStates$InternetGatewayBlockMode`), which is the state of an
/// account that has never enabled it.
pub const BLOCK_PUBLIC_ACCESS_OFF: &str = "off";

/// Per-service state for com-amazonaws-ec2-amazonec2. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsEc2Amazonec2State {
    /// Every VPC the account holds, by `VpcId`.
    ///
    /// Availability Zones are NOT here: they are derived from the Region (see the
    /// module docs). Further caller-created resources go beside this field as they
    /// land.
    pub vpcs: BTreeMap<String, Vpc>,
    /// Monotonic source of generated resource-id suffixes, so an id is unique within a
    /// run and deterministic across runs.
    pub id_counter: u64,
}

/// A fresh account's EC2 state: the default VPC AWS creates for it, and nothing else.
impl Default for ComAmazonawsEc2Amazonec2State {
    fn default() -> Self {
        let mut state = Self {
            vpcs: BTreeMap::new(),
            id_counter: 0,
        };
        state.insert_vpc(
            DEFAULT_VPC_CIDR_BLOCK,
            true,
            DEFAULT_INSTANCE_TENANCY,
            VPC_STATE_AVAILABLE,
            Vec::new(),
        );
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
    pub fn matches(&self, value: &str) -> bool {
        let tokens = &self.tokens;
        let value: Vec<char> = value.chars().collect();
        // Greedy match with one backtrack point per `*`, which is all a pattern with no
        // alternation needs: on a mismatch, the most recent `*` consumes one more
        // character and matching resumes after it.
        let (mut token, mut position) = (0usize, 0usize);
        let mut backtrack: Option<(usize, usize)> = None;
        while position < value.len() {
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
