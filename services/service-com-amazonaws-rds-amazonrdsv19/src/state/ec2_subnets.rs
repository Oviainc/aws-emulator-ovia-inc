// SPDX-License-Identifier: CC0-1.0

//! The Availability Zone each subnet a request names lies in, as the service that OWNS
//! subnets reports it.
//!
//! A subnet is not a resource of this service: it is Amazon EC2's. A client builds its
//! network with `ec2:CreateVpc`/`ec2:CreateSubnet`, or uses the default VPC EC2 materialises
//! for every Region of the account, and then names those subnets in `SubnetIds`. So EC2 is
//! the ONE answer to "which zone is this subnet in", and an identifier a request names is
//! resolved there and nowhere else — reached the way any emulated service reaches another,
//! through the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so this crate names no other
//! service's crate and nothing leaves the process.
//!
//! What a DB subnet group reports about its subnets — the zone each one lies in, and so the
//! zones the group COVERS, and the VPC that owns them, which is the group's own — is read off
//! the records EC2 answered with, never built from the identifiers themselves: a subnet's
//! zone and VPC are EC2's facts, and a client cross-checking `DescribeDBSubnetGroups` against
//! `ec2:DescribeSubnets` must see one answer, not two.
//!
//! An identifier EC2 ANSWERED it holds no subnet for names no subnet of the account, and a
//! request carrying one is refused: the subnets a DB subnet group would hold have to be
//! subnets, and a later `ec2:DescribeSubnets` could agree with no group built on an
//! identifier nobody minted.
//!
//! A hop that could not be made at all (no EC2 in this process, an answer that could not be
//! read) is evidence of nothing: the zone of the identifiers it would have answered for
//! stays UNKNOWN, which this service reports as no zone rather than as a zone of its own
//! choosing, and which neither a refusal nor a zone is assumed from
//! (`services/common/src/network.rs` states which reading belongs to which kind of
//! service).
//!
//! Nothing is cached: a subnet created a moment ago resolves at once, and one that was
//! deleted stops resolving at once.

use std::collections::{BTreeMap, BTreeSet};

/// The `DescribeSubnets` filter that selects subnets BY ID (`subnet-id`, listed by that
/// operation's own `Filters` documentation).
///
/// A filter is what asks the question rather than `SubnetIds`: an identifier the account has
/// no subnet for is then an absent row instead of a failure of the whole call, so every
/// identifier the request named is still resolved against what EC2 does hold.
const SUBNET_ID_FILTER: &str = "subnet-id";

/// The most subnet identifiers ONE hop asks about, as values of one `subnet-id` filter.
///
/// The owner evaluates a filter against every subnet the account stores, so the cost of a
/// hop is (identifiers asked x subnets stored): this caps the first factor, so one hop costs
/// a constant times what the account stores rather than something the request chooses. A DB
/// subnet group holds at most [`super::MAX_SUBNETS_PER_DB_SUBNET_GROUP`] subnets, so a
/// create normally fits in one hop.
const SUBNET_IDS_PER_HOP: usize = 64;

/// What a request naming subnets that cannot make up a DB subnet group — ones the account
/// does not hold, or ones outside the group's VPC — is refused with.
///
/// Amazon RDS states the identifiers it could not use inside the brackets, comma-separated
/// and in the order the request named them, and leaves the fixed text around them, whose
/// spacing (the space before the colon, none after it) is AWS's own.
pub fn invalid_subnets_message(subnet_ids: &[String]) -> String {
    format!(
        "Some input subnets in :[{}] are invalid.",
        subnet_ids.join(", ")
    )
}

/// An AWS SDK client for EC2 over the in-process callback into this same emulator.
///
/// `region` is the request's own signing region, so the question is asked in the Region the
/// caller addressed. The endpoint URL is only the address the callback's transport routes on
/// IN PROCESS: no socket is opened, no name is resolved and nothing leaves this process. The
/// credentials are the callback's, which sign this in-process hop and never appear in a
/// response.
///
/// The Region a request is signed for is CALLER-SUPPLIED text and here it ends up in an
/// endpoint URL's authority, so it is read as a Region LABEL
/// (`service_common::arn::region_label`, the one place this tree states that grammar)
/// instead of being interpolated as given: `None` for a value that would name an authority
/// the caller chose rather than the peer service this emulator asks.
///
/// `pub(super)` so the sibling module that resolves the VPC security groups a request names
/// ([`super::ec2_security_groups`]) asks EC2 through this one client rather than building a
/// second one.
pub(super) fn ec2(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Option<::aws_sdk_ec2::Client> {
    let region = ::service_common::arn::region_label(region)?;
    let config = ::aws_sdk_ec2::Config::builder()
        .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_ec2::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://ec2.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client is
        // not given. The same goes for caching the identity: the credentials the callback
        // hands over are used directly.
        .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled())
        .build();
    Some(::aws_sdk_ec2::Client::from_conf(config))
}

/// What the owner of subnets answered about the identifiers a request named: the
/// Availability Zone of each one it holds a subnet for.
///
/// An identifier that is absent here is one the account has no subnet for, or one asked
/// about in a process EC2 is not mounted in: in both cases this service knows no zone for
/// it, which it reports as no zone.
#[derive(Debug, Clone, Default)]
pub struct SubnetZones {
    /// `subnet-id` -> the `AvailabilityZone` EC2 reports for it.
    by_id: BTreeMap<String, String>,
    /// `subnet-id` -> the `VpcId` EC2 reports the subnet belongs to.
    vpc_by_id: BTreeMap<String, String>,
    /// Every identifier EC2 ANSWERED with a subnet for, whichever of that subnet's own
    /// members the record carried: whether the account HOLDS the subnet is its own fact,
    /// never inferred from the zone or the VPC the record happened to name.
    held: BTreeSet<String>,
    /// Whether the owner of subnets ANSWERED every question asked of it. `false` where no
    /// hop could be made at all, which is the one case an absent entry says nothing about
    /// whether the account holds the subnet — so it defaults to `false`, and nothing is
    /// refused on a hop that was never answered.
    answered: bool,
}

impl SubnetZones {
    /// The zone EC2 reports the subnet `subnet_id` lies in, or `None` where it answered for
    /// no such subnet.
    #[must_use]
    pub fn zone_of(&self, subnet_id: &str) -> Option<&str> {
        self.by_id.get(subnet_id).map(String::as_str)
    }

    /// The zones `subnet_ids` COVER: the distinct zones of the subnets among them EC2
    /// answered for, named in zone order.
    #[must_use]
    pub fn covered<'ids>(
        &self,
        subnet_ids: impl IntoIterator<Item = &'ids str>,
    ) -> BTreeSet<String> {
        subnet_ids
            .into_iter()
            .filter_map(|subnet_id| self.zone_of(subnet_id).map(str::to_string))
            .collect()
    }

    /// How many of `subnet_ids` resolved to no subnet of the account, and so lie in a zone
    /// this service does not know.
    #[must_use]
    pub fn unresolved<'ids>(&self, subnet_ids: impl IntoIterator<Item = &'ids str>) -> usize {
        subnet_ids
            .into_iter()
            .filter(|subnet_id| self.zone_of(subnet_id).is_none())
            .count()
    }

    /// The identifiers among `subnet_ids` that EC2 ANSWERED it holds no subnet for, in the
    /// order the request named them and without repetition.
    ///
    /// EMPTY where the hop could not be made or the owner answered with an error: this
    /// service then knows nothing about the subnets, and a request is refused on what EC2
    /// said rather than on silence.
    #[must_use]
    pub fn absent(&self, subnet_ids: &[String]) -> Vec<String> {
        if !self.answered {
            return Vec::new();
        }
        let mut named: BTreeSet<&str> = BTreeSet::new();
        subnet_ids
            .iter()
            .filter(|subnet_id| !self.held.contains(subnet_id.as_str()))
            .filter(|subnet_id| named.insert(subnet_id.as_str()))
            .cloned()
            .collect()
    }

    /// The identifiers among `subnet_ids` EC2 reports in a VPC OTHER than `vpc_id`, in the
    /// order the request named them and without repetition.
    ///
    /// An identifier EC2 named no VPC for is not among them: a subnet this service knows no
    /// VPC for is no evidence of a subnet in the wrong one.
    #[must_use]
    pub fn outside_vpc(&self, subnet_ids: &[String], vpc_id: &str) -> Vec<String> {
        let mut named: BTreeSet<&str> = BTreeSet::new();
        subnet_ids
            .iter()
            .filter(|subnet_id| {
                self.vpc_by_id
                    .get(subnet_id.as_str())
                    .is_some_and(|owner| owner != vpc_id)
            })
            .filter(|subnet_id| named.insert(subnet_id.as_str()))
            .cloned()
            .collect()
    }

    /// The distinct VPCs EC2 reported for `subnet_ids`, in id order.
    #[must_use]
    pub fn vpcs<'ids>(&self, subnet_ids: impl IntoIterator<Item = &'ids str>) -> BTreeSet<String> {
        subnet_ids
            .into_iter()
            .filter_map(|subnet_id| self.vpc_by_id.get(subnet_id).cloned())
            .collect()
    }

    /// The VPC that owns `subnet_ids`: the one EC2 reported for every one of them.
    ///
    /// `None` where it reported none, or reported more than one — a group whose subnets lie
    /// in different VPCs has no VPC of its own, and neither has one asked about where the
    /// owner of subnets could not be reached.
    #[must_use]
    pub fn vpc_of<'ids>(&self, subnet_ids: impl IntoIterator<Item = &'ids str>) -> Option<String> {
        let mut owners = self.vpcs(subnet_ids).into_iter();
        match (owners.next(), owners.next()) {
            (Some(vpc_id), None) => Some(vpc_id),
            _ => None,
        }
    }

    /// The zones, by subnet identifier, a DB subnet group KEEPS for the subnets it was
    /// created or modified with: what its `Subnets` report, read off EC2's records at the
    /// moment the group was written.
    #[must_use]
    pub fn into_zones_by_subnet(self) -> BTreeMap<String, String> {
        self.by_id
    }
}

/// The Availability Zone of every identifier in `subnet_ids` the account has a subnet for,
/// in `region`, as EC2 reports it.
///
/// Identifiers are deduplicated, so the work does not grow with how many times one subnet is
/// named, and asked about in hops of at most [`SUBNET_IDS_PER_HOP`]. The CALLER has already
/// held the list to the published allowance for one DB subnet group
/// ([`super::check_db_subnet_group_subnet_list`], which runs before this), so the number of
/// hops is bounded by that allowance and not by the length of list a caller sent.
///
/// A hop that could not be made contributes nothing, and the caller treats the zone of those
/// identifiers as unknown: this service GATES a subnet list on the zones it covers and
/// REPORTS the zone of each subnet, and neither an assumed zone nor an assumed refusal is
/// grounded in anything EC2 said.
pub async fn subnet_zones(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[String],
) -> SubnetZones {
    let mut zones = SubnetZones::default();
    let Some(client) = ec2(ctx, region) else {
        ::tracing::warn!("the request names no Region label, so EC2 cannot be asked");
        return zones;
    };
    // Every hop below has to be answered for an absent entry to mean "the account holds no
    // such subnet"; one that fails takes this back off.
    zones.answered = true;
    let distinct: Vec<&str> = subnet_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<&str>>()
        .into_iter()
        .collect();
    for chunk in distinct.chunks(SUBNET_IDS_PER_HOP) {
        let by_id = ::aws_sdk_ec2::types::Filter::builder()
            .name(SUBNET_ID_FILTER)
            .set_values(Some(chunk.iter().map(|id| (*id).to_string()).collect()))
            .build();
        match client.describe_subnets().filters(by_id).send().await {
            Ok(output) => {
                for subnet in output.subnets() {
                    let Some(subnet_id) = subnet.subnet_id() else {
                        continue;
                    };
                    // The account HOLDS this subnet: recorded on its own, so an id EC2
                    // answered for is never reported absent because the record named no
                    // zone or no VPC.
                    zones.held.insert(subnet_id.to_string());
                    if let Some(zone) = subnet.availability_zone() {
                        zones
                            .by_id
                            .insert(subnet_id.to_string(), zone.to_string());
                    }
                    // The VPC the subnet belongs to comes off the same record, for the same
                    // reason: it is the owner of subnets' fact, and it is the group's own VPC.
                    if let Some(vpc_id) = subnet.vpc_id() {
                        zones
                            .vpc_by_id
                            .insert(subnet_id.to_string(), vpc_id.to_string());
                    }
                }
            }
            Err(error) => {
                // At `warn`: a group's zones are what a client reads to tell which zones it
                // already covers, so a deployment where this hop fails is one where no
                // group can report them — worth seeing, and it IS why an identifier
                // resolved to no zone.
                ::tracing::warn!(
                    %region,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the subnets a request named could not be resolved against EC2"
                );
                zones.answered = false;
            }
        }
    }
    zones
}
