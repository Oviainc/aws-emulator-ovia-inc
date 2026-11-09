// SPDX-License-Identifier: CC0-1.0

//! The account's VPC network: facts this service RESOLVES rather than holds.
//!
//! A load balancer is placed in subnets, a target group belongs to a VPC and an `ip`
//! target is an address inside one — none of which is a resource of this service. They
//! live in EC2, so what a request names is resolved against EC2, reached the way any
//! emulated service reaches another: through the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so this crate names no other
//! service's crate.
//!
//! Every refusal built here is grounded in what EC2 ANSWERED. An emulator with no EC2
//! mounted knows nothing about the subnet, the VPC or the instance, and nothing is what
//! it refuses on: a refusal is never assumed from silence, and a member that cannot be
//! resolved is left unreported rather than invented.

use std::net::IpAddr;

use ipnet::IpNet;

/// One subnet of the account, reduced to what Elastic Load Balancing reads off it: which
/// VPC and Availability Zone it is in, and which addresses it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubnetFacts {
    pub subnet_id: String,
    pub vpc_id: Option<String>,
    pub availability_zone: Option<String>,
    /// The subnet's IPv4 and IPv6 CIDR blocks.
    pub cidrs: Vec<IpNet>,
}

impl SubnetFacts {
    /// Whether EC2 reported an IPv6 range for this subnet, which is where a load balancer
    /// placed in it draws its IPv6 addresses from.
    pub fn has_ipv6_cidr(&self) -> bool {
        self.cidrs.iter().any(|cidr| matches!(cidr, IpNet::V6(_)))
    }
}

/// What EC2 says about an instance a request named as a target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstanceLookup {
    /// EC2 reports the instance, and (when it carries one) the VPC it is in.
    Found { vpc_id: Option<String> },
    /// EC2 reports that the account holds no instance with that id — including an id
    /// that is not an instance id at all, which names no instance either.
    Absent,
    /// Nothing is known: there is no EC2 to ask, or it answered something else.
    Unknown,
}

/// The address ranges an `ip` target may come from BESIDES the subnets of the target
/// group's own VPC, from `TargetType`'s own documentation: "ip - Register targets by IP
/// address. You can specify IP addresses from the subnets of the virtual private cloud
/// (VPC) for the target group, the RFC 1918 range (10.0.0.0/8, 172.16.0.0/12, and
/// 192.168.0.0/16), and the RFC 6598 range (100.64.0.0/10). You can't specify publicly
/// routable IP addresses."
///
/// The IPv6 row is the unique-local range RFC 4193 reserves (`fc00::/7`), which is the
/// IPv6 equivalent of those ranges: an IPv6 target group's addresses are likewise not
/// publicly routable ones.
const PRIVATE_TARGET_RANGES: &[&str] = &[
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "100.64.0.0/10",
    "fc00::/7",
];

/// Those ranges as networks, parsed ONCE: a value this table cannot be read as a network
/// is a typo in the table rather than something to skip, so it panics here instead of
/// silently widening what an `ip` target may be.
fn private_target_ranges() -> &'static [IpNet] {
    static PARSED: std::sync::OnceLock<Vec<IpNet>> = std::sync::OnceLock::new();
    PARSED.get_or_init(|| {
        PRIVATE_TARGET_RANGES
            .iter()
            .map(|range| {
                range
                    .parse::<IpNet>()
                    .unwrap_or_else(|_| panic!("{range} is a CIDR block"))
            })
            .collect()
    })
}

/// An AWS SDK client over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated service:
/// it routes in process, so no network client is configured here. `region` is the
/// request's own signing region.
macro_rules! callback_client {
    ($sdk:ident, $ctx:expr, $endpoint:expr, $region:expr) => {{
        let config = ::$sdk::Config::builder()
            .behavior_version(::$sdk::config::BehaviorVersion::latest())
            .http_client($ctx.callback().http_client())
            .credentials_provider($ctx.callback().aws_credentials_provider())
            .region(::$sdk::config::Region::new($region.to_string()))
            .endpoint_url($endpoint)
            // The transport is in-process: there is nothing to retry, time out or watch
            // for a stalled stream, and each of those would need a sleep implementation
            // this client is not given. The same goes for caching the identity: the
            // credentials are handed over directly.
            .retry_config(::$sdk::config::retry::RetryConfig::disabled())
            .timeout_config(::$sdk::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::$sdk::config::IdentityCache::no_cache())
            .stalled_stream_protection(::$sdk::config::StalledStreamProtectionConfig::disabled())
            .build();
        ::$sdk::Client::from_conf(config)
    }};
}

/// The EC2 client this service resolves the account's network through.
fn ec2(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_ec2::Client {
    callback_client!(
        aws_sdk_ec2,
        ctx,
        format!("https://ec2.{region}.amazonaws.com"),
        region
    )
}

/// The CIDR blocks of one EC2 subnet, parsed; a block EC2 reports in a form this emulator
/// cannot read as a network is dropped rather than guessed at.
fn subnet_cidrs(subnet: &::aws_sdk_ec2::types::Subnet) -> Vec<IpNet> {
    let ipv6 = subnet
        .ipv6_cidr_block_association_set()
        .iter()
        .filter_map(|association| association.ipv6_cidr_block());
    subnet
        .cidr_block()
        .into_iter()
        .chain(ipv6)
        .filter_map(|cidr| cidr.parse::<IpNet>().ok())
        .collect()
}

/// EC2's answer about `subnet` as the facts this service reads off it.
fn subnet_facts(subnet: &::aws_sdk_ec2::types::Subnet) -> SubnetFacts {
    SubnetFacts {
        subnet_id: subnet.subnet_id().unwrap_or_default().to_string(),
        vpc_id: subnet.vpc_id().map(str::to_string),
        availability_zone: subnet.availability_zone().map(str::to_string),
        cidrs: subnet_cidrs(subnet),
    }
}

/// The EC2 refusal a read of the account's network ran into.
///
/// Elastic Load Balancing reports the refusal EC2 answered rather than restating it, so a
/// caller that refuses its own request states one of these: the error CODE EC2 answered,
/// and — when EC2 worded a sentence of its own — that sentence, which names the id EC2
/// refused the read for and so tells the caller WHICH of the values it sent was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkRefusal {
    /// The error code EC2 answered.
    pub code: String,
    /// The sentence EC2 worded the refusal with, or `None` when it answered none.
    pub message: Option<String>,
    /// The HTTP status EC2 answered, when its answer reached this service at all.
    pub status: Option<u16>,
}

impl NetworkRefusal {
    /// A refusal EC2 answered no sentence of its own for: a read that resolved without
    /// reporting one of the ids says what the code says, and nothing more.
    pub fn new(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: None,
            status: None,
        }
    }

    /// The refusal EC2 answered, with the sentence it worded (when it worded one).
    pub fn worded(code: impl Into<String>, message: Option<String>) -> Self {
        Self::answered(code, message, None)
    }

    /// The refusal EC2 ANSWERED on the wire: its sentence and the status it came with,
    /// which a caller reporting the failure states beside the code
    /// ([`NetworkRefusal::reported_message`]).
    pub fn answered(code: impl Into<String>, message: Option<String>, status: Option<u16>) -> Self {
        Self {
            code: code.into(),
            message,
            status,
        }
    }

    /// What a caller states as the MESSAGE of its own refusal: EC2's sentence when EC2
    /// worded one, because it names the id at fault, and the code it answered otherwise.
    ///
    /// A refusal a caller REPORTS to its own client is built with a sentence of its own
    /// ([`unresolved_ids_message`]) for the case EC2 worded none, so the code fallback
    /// here is the last resort for a refusal nobody worded: an error message is a
    /// sentence about the request, and a bare error code states nothing a caller can act
    /// on.
    pub fn reported_message(&self) -> String {
        self.message
            .clone()
            .unwrap_or_else(|| self.code.clone())
    }

    /// What the Elastic Load Balancing v2 API states as the MESSAGE of its OWN refusal for
    /// an EC2-owned resource it could not resolve: the error CODE EC2 answered, and nothing
    /// else.
    ///
    /// That is this API's own wording for the class — the refusal is its `SubnetNotFound`
    /// or `AllocationIdNotFound` rather than EC2's, so neither EC2's sentence, nor the
    /// service and status it answered, nor the id of the call that was made is part of it.
    /// A caller reads the inner code, which says WHICH kind of resource was not resolved,
    /// under this API's own error code.
    ///
    /// Which of the two forms a service states is the service's own: the Classic API states
    /// the inner SENTENCE instead ([`NetworkRefusal::reported_message`]).
    pub fn stated_message(&self) -> String {
        self.code.clone()
    }
}

/// What EC2 says about the subnets a request named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubnetLookup {
    /// EC2 reported the subnets, in the order it answered.
    Resolved(Vec<SubnetFacts>),
    /// EC2 reports that the account holds no subnet for one of the ids the request named,
    /// carrying the refusal it answered (`InvalidSubnetID.NotFound` for an id that
    /// names no subnet, `InvalidSubnetID.Malformed` for a value that is not a subnet id at
    /// all) — which is what a caller states when it refuses the request, because Elastic
    /// Load Balancing reports the EC2 refusal it ran into rather than restating it.
    Absent(NetworkRefusal),
    /// Nothing is known: there is no EC2 to ask, or it answered something else.
    Unknown,
}

/// What EC2 says about the subnets `subnet_ids` names.
///
/// EC2 refuses a read naming an id the account holds no subnet for, so one bad id says
/// nothing about the others — which is why the refusal is reported for the request as a
/// whole. Any other failure is this emulator failing to ask rather than EC2 reporting a bad
/// subnet, so it says nothing ([`SubnetLookup::Unknown`]), and a request naming no subnet
/// at all resolves to no facts.
pub async fn resolve_subnets(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[String],
) -> SubnetLookup {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    if subnet_ids.is_empty() {
        return SubnetLookup::Unknown;
    }
    let answer = ec2(ctx, region)
        .describe_subnets()
        .set_subnet_ids(Some(subnet_ids.to_vec()))
        .send()
        .await;
    let error = match answer {
        Ok(output) => {
            let reported: Vec<SubnetFacts> = output.subnets().iter().map(subnet_facts).collect();
            // A read that resolved but reports no subnet for an id the request named says
            // the same thing the refusal below does.
            let unresolved: Vec<&str> = subnet_ids
                .iter()
                .filter(|id| !reported.iter().any(|subnet| &&subnet.subnet_id == id))
                .map(String::as_str)
                .collect();
            if !unresolved.is_empty() {
                return SubnetLookup::Absent(NetworkRefusal::worded(
                    SUBNET_ID_NOT_FOUND,
                    Some(unresolved_ids_message(SUBNET_ID_NOUN, &unresolved)),
                ));
            }
            return SubnetLookup::Resolved(reported);
        }
        Err(error) => error,
    };
    let refused = error.as_service_error();
    let code = refused
        .and_then(ProvideErrorMetadata::code)
        .unwrap_or_default()
        .to_string();
    if code.starts_with(SUBNET_ID_REFUSAL_PREFIX) {
        // EC2 worded this refusal around the subnet id it would not resolve, so the
        // sentence is carried along with the code: it is what names that id to the caller.
        // EC2 refused the read for the ids it was given as a whole, so when it worded no
        // sentence of its own the refusal states them.
        let detail = refused
            .and_then(ProvideErrorMetadata::message)
            .map(str::to_string)
            .unwrap_or_else(|| {
                let named: Vec<&str> = subnet_ids.iter().map(String::as_str).collect();
                unresolved_ids_message(SUBNET_ID_NOUN, &named)
            });
        return SubnetLookup::Absent(NetworkRefusal::answered(
            code,
            Some(detail),
            error.raw_response().map(|response| response.status().as_u16()),
        ));
    }
    ::tracing::debug!(
        error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
        "the subnets a request named could not be resolved against EC2"
    );
    SubnetLookup::Unknown
}

/// The prefix of every EC2 error code that says an id names no subnet of the account:
/// `InvalidSubnetID.NotFound` for one no subnet carries, `InvalidSubnetID.Malformed` for a
/// value that is not a subnet id at all. Both say the same thing about a placement.
const SUBNET_ID_REFUSAL_PREFIX: &str = "InvalidSubnetID.";

/// The EC2 code for an id the account holds no subnet for, which is what a read that
/// resolved without reporting one of the ids ran into.
const SUBNET_ID_NOT_FOUND: &str = "InvalidSubnetID.NotFound";

/// What a refusal CALLS a subnet id, for the sentence it states when EC2 worded none.
const SUBNET_ID_NOUN: &str = "subnet ID";

/// What a refusal CALLS an Elastic IP allocation id, for the same sentence.
const ALLOCATION_ID_NOUN: &str = "allocation ID";

/// The sentence a refusal for ids the account holds no resource for states: the kind of
/// id, and every id of that kind the read could not resolve.
///
/// An error reply's message is a SENTENCE about the request — it is what tells a caller
/// which of the values it sent was refused — so a refusal reported to a client never
/// states a bare error code. The ids come from the REQUEST, and the clause agrees in
/// number with how many of them were refused.
fn unresolved_ids_message(noun: &str, ids: &[&str]) -> String {
    let quoted: Vec<String> = ids.iter().map(|id| format!("'{id}'")).collect();
    match quoted.as_slice() {
        [one] => format!("The {noun} {one} does not exist"),
        many => format!("The {noun}s {} do not exist", many.join(", ")),
    }
}

/// The prefix of every EC2 error code that says a value names no Elastic IP allocation of
/// the account: `InvalidAllocationID.NotFound` and `InvalidAllocationID.Malformed`.
const ALLOCATION_ID_REFUSAL_PREFIX: &str = "InvalidAllocationID.";

/// The EC2 code for an id the account holds no Elastic IP allocation for.
const ALLOCATION_ID_NOT_FOUND: &str = "InvalidAllocationID.NotFound";

/// The EC2 refusal the allocation ids a request pinned ran into, or `None` when EC2
/// resolved every one of them — and also when there is no EC2 to ask, because a refusal has
/// to be grounded in what EC2 reported rather than assumed from silence.
///
/// An Elastic IP address is EC2's resource, and its allocation id is what names it, so
/// whether an address exists to pin on a load balancer's subnet is EC2's to say. The value
/// is the refusal EC2 answered — its error CODE, and the sentence that names the
/// allocation id at fault — for the caller to state in its own refusal.
pub async fn absent_allocation_id(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    allocation_ids: &[String],
) -> Option<NetworkRefusal> {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    if allocation_ids.is_empty() {
        return None;
    }
    let answer = ec2(ctx, region)
        .describe_addresses()
        .set_allocation_ids(Some(allocation_ids.to_vec()))
        .send()
        .await;
    let error = match answer {
        Ok(output) => {
            let reported: Vec<&str> = output
                .addresses()
                .iter()
                .filter_map(::aws_sdk_ec2::types::Address::allocation_id)
                .collect();
            let unresolved: Vec<&str> = allocation_ids
                .iter()
                .map(String::as_str)
                .filter(|id| !reported.contains(id))
                .collect();
            return (!unresolved.is_empty()).then(|| {
                NetworkRefusal::worded(
                    ALLOCATION_ID_NOT_FOUND,
                    Some(unresolved_ids_message(ALLOCATION_ID_NOUN, &unresolved)),
                )
            });
        }
        Err(error) => error,
    };
    let refused = error.as_service_error();
    let code = refused
        .and_then(ProvideErrorMetadata::code)
        .unwrap_or_default()
        .to_string();
    if code.starts_with(ALLOCATION_ID_REFUSAL_PREFIX) {
        // EC2's own sentence names the allocation id it would not resolve; when it worded
        // none, the refusal states the ids the read named.
        let detail = refused
            .and_then(ProvideErrorMetadata::message)
            .map(str::to_string)
            .unwrap_or_else(|| {
                let named: Vec<&str> = allocation_ids.iter().map(String::as_str).collect();
                unresolved_ids_message(ALLOCATION_ID_NOUN, &named)
            });
        return Some(NetworkRefusal::answered(
            code,
            Some(detail),
            error.raw_response().map(|response| response.status().as_u16()),
        ));
    }
    ::tracing::debug!(
        error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
        "the Elastic IP allocations a request named could not be resolved against EC2"
    );
    None
}

/// The subnets EC2 holds for the ids a request named, in the order EC2 answered, or
/// `None` when EC2 could not be asked or refused the read (an id that names no subnet of
/// the account refuses the WHOLE read, so nothing is known about any of them).
///
/// The reporting half of [`resolve_subnets`], for a caller that places nothing and so has
/// no refusal to raise: an unresolved subnet leaves the members it would have filled
/// unreported.
pub async fn subnets_by_id(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[String],
) -> Option<Vec<SubnetFacts>> {
    match resolve_subnets(ctx, region, subnet_ids).await {
        SubnetLookup::Resolved(subnets) => Some(subnets),
        SubnetLookup::Absent(_) | SubnetLookup::Unknown => None,
    }
}

/// The Availability Zone ID EC2 reports for each of `subnet_ids`, keyed by subnet id.
///
/// An Availability Zone has TWO identifiers — the account-scoped name (`us-west-2a`) and
/// the account-independent ID (`usw2-az1`) — and only EC2 holds the pairing, so a service
/// that has to name the zone a subnet puts a resource in by its ID reads it from EC2 rather
/// than deriving it. A subnet EC2 does not report, or reports no zone ID for, is simply
/// left out, and an answer that could not be obtained at all is an empty map: the caller
/// then has only the zone NAME it already holds, which is what it falls back to.
pub async fn zone_ids_of_subnets(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[String],
) -> std::collections::BTreeMap<String, String> {
    if subnet_ids.is_empty() {
        return std::collections::BTreeMap::new();
    }
    let answer = ec2(ctx, region)
        .describe_subnets()
        .set_subnet_ids(Some(subnet_ids.to_vec()))
        .send()
        .await;
    match answer {
        Ok(output) => output
            .subnets()
            .iter()
            .filter_map(|subnet| {
                Some((
                    subnet.subnet_id()?.to_string(),
                    subnet.availability_zone_id()?.to_string(),
                ))
            })
            .collect(),
        Err(error) => {
            ::tracing::debug!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the Availability Zone IDs of the subnets could not be resolved against EC2"
            );
            std::collections::BTreeMap::new()
        }
    }
}

/// Every subnet EC2 holds in `vpc_id`, or `None` when EC2 could not be asked.
pub async fn subnets_of_vpc(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    vpc_id: &str,
) -> Option<Vec<SubnetFacts>> {
    let filter = ::aws_sdk_ec2::types::Filter::builder()
        .name("vpc-id")
        .values(vpc_id)
        .build();
    let answer = ec2(ctx, region)
        .describe_subnets()
        .filters(filter)
        .send()
        .await;
    match answer {
        Ok(output) => Some(output.subnets().iter().map(subnet_facts).collect()),
        Err(error) => {
            ::tracing::debug!(
                %vpc_id,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the subnets of the VPC could not be resolved against EC2"
            );
            None
        }
    }
}

/// The CIDR blocks EC2 holds for `vpc_id`, or `None` when EC2 could not be asked or holds
/// no such VPC.
pub async fn vpc_cidrs(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    vpc_id: &str,
) -> Option<Vec<IpNet>> {
    let answer = ec2(ctx, region)
        .describe_vpcs()
        .vpc_ids(vpc_id)
        .send()
        .await;
    let output = match answer {
        Ok(output) => output,
        Err(error) => {
            ::tracing::debug!(
                %vpc_id,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the VPC a request named could not be resolved against EC2"
            );
            return None;
        }
    };
    let vpc = output.vpcs().first()?;
    let ipv4 = vpc
        .cidr_block_association_set()
        .iter()
        .filter_map(|association| association.cidr_block())
        .chain(vpc.cidr_block());
    let ipv6 = vpc
        .ipv6_cidr_block_association_set()
        .iter()
        .filter_map(|association| association.ipv6_cidr_block());
    let mut cidrs: Vec<IpNet> = ipv4
        .chain(ipv6)
        .filter_map(|cidr| cidr.parse::<IpNet>().ok())
        .collect();
    cidrs.sort_by_key(ToString::to_string);
    cidrs.dedup();
    Some(cidrs)
}

/// One security group of the account, reduced to what Elastic Load Balancing reads off it:
/// which VPC it governs traffic in, and the name and owner a load balancer reports it
/// under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityGroupFacts {
    pub group_id: String,
    pub group_name: Option<String>,
    /// The account that owns the group, which is the alias a load balancer reports its
    /// source security group under.
    pub owner_id: Option<String>,
    pub vpc_id: Option<String>,
}

/// What EC2 says about the security groups a request named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecurityGroupLookup {
    /// EC2 reported every group the request named, in the order it answered.
    Resolved(Vec<SecurityGroupFacts>),
    /// EC2 reports that the account holds no security group for one of the ids the request
    /// named, carrying the refusal it answered — which is what a caller states when it
    /// refuses the request, because Elastic Load Balancing reports the EC2 refusal it ran
    /// into rather than restating it.
    Absent(NetworkRefusal),
    /// Nothing is known: there is no EC2 to ask, or it answered something else.
    Unknown,
}

/// The two EC2 error codes that say a value names no security group of the account:
/// `InvalidGroup.NotFound` for an id no group of the account carries, and
/// `InvalidGroupId.Malformed` for a value that is not a group id at all. Both say the same
/// thing about an association, and no OTHER `InvalidGroup…` code does — a group that
/// exists but cannot be used says something else — so the two are matched exactly rather
/// than by prefix.
const GROUP_REFUSAL_CODES: &[&str] = &[GROUP_NOT_FOUND, GROUP_ID_MALFORMED];

/// The EC2 code for a value that is not a security group id at all, which says the same
/// thing about an association as [`GROUP_NOT_FOUND`]: nothing the request named is a group
/// this account can use.
const GROUP_ID_MALFORMED: &str = "InvalidGroupId.Malformed";

/// The EC2 code for an id the account holds no security group for, which is what a read
/// that resolved without reporting one of the ids ran into.
const GROUP_NOT_FOUND: &str = "InvalidGroup.NotFound";

/// What a refusal CALLS a security group id, for the sentence it states when EC2 worded
/// none — EC2's own noun for the resource an id of this kind names.
const GROUP_NOUN: &str = "security group";

/// What EC2 says about the security groups `group_ids` names.
///
/// EC2 refuses a read naming an id the account holds no group for, so one bad id says
/// nothing about the others — which is why the refusal is reported for the request as a
/// whole. Any other failure is this emulator failing to ask rather than EC2 reporting a bad
/// group, so it says nothing ([`SecurityGroupLookup::Unknown`]), which is also the answer
/// for a request that names no group at all: there was nothing to ask about, so nothing is
/// known and nothing is refused on it.
pub async fn resolve_security_groups(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    group_ids: &[String],
) -> SecurityGroupLookup {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    if group_ids.is_empty() {
        return SecurityGroupLookup::Unknown;
    }
    let answer = ec2(ctx, region)
        .describe_security_groups()
        .set_group_ids(Some(group_ids.to_vec()))
        .send()
        .await;
    let error = match answer {
        Ok(output) => {
            let reported: Vec<SecurityGroupFacts> = output
                .security_groups()
                .iter()
                .map(|group| SecurityGroupFacts {
                    group_id: group.group_id().unwrap_or_default().to_string(),
                    group_name: group.group_name().map(str::to_string),
                    owner_id: group.owner_id().map(str::to_string),
                    vpc_id: group.vpc_id().map(str::to_string),
                })
                .collect();
            // A read that resolved but reports no group for an id the request named says
            // the same thing the refusal below does. The reported ids are indexed FIRST, so
            // this costs O(n log n) in the number of ids the request carried rather than
            // the product of the two caller-sized lists.
            let index: std::collections::BTreeSet<&str> = reported
                .iter()
                .map(|group| group.group_id.as_str())
                .collect();
            if group_ids.iter().any(|id| !index.contains(id.as_str())) {
                let unresolved: Vec<&str> = group_ids
                    .iter()
                    .map(String::as_str)
                    .filter(|id| !index.contains(id))
                    .collect();
                return SecurityGroupLookup::Absent(NetworkRefusal::worded(
                    GROUP_NOT_FOUND,
                    Some(unresolved_ids_message(GROUP_NOUN, &unresolved)),
                ));
            }
            return SecurityGroupLookup::Resolved(reported);
        }
        Err(error) => error,
    };
    let refused = error.as_service_error();
    let code = refused
        .and_then(ProvideErrorMetadata::code)
        .unwrap_or_default()
        .to_string();
    if GROUP_REFUSAL_CODES.contains(&code.as_str()) {
        // EC2 worded this refusal around the group id it would not resolve, so the
        // sentence is carried along with the code: it is what names that id to the caller.
        // EC2 refused the read for the ids it was given as a whole, so when it worded no
        // sentence of its own the refusal states them. The status EC2 ANSWERED travels with
        // them, as it does for every other resource this module resolves: a caller that
        // states the fault states what EC2 answered rather than a status of its own.
        let message = refused
            .and_then(ProvideErrorMetadata::message)
            .map(str::to_string)
            .unwrap_or_else(|| {
                let named: Vec<&str> = group_ids.iter().map(String::as_str).collect();
                unresolved_ids_message(GROUP_NOUN, &named)
            });
        return SecurityGroupLookup::Absent(NetworkRefusal::answered(
            code,
            Some(message),
            error.raw_response().map(|response| response.status().as_u16()),
        ));
    }
    ::tracing::debug!(
        error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
        "the security groups a request named could not be resolved against EC2"
    );
    SecurityGroupLookup::Unknown
}

/// The name EC2 gives the security group every VPC comes with: "your VPC automatically
/// comes with a default security group" named `default` (Amazon VPC User Guide, "Default
/// security groups for your VPCs").
const DEFAULT_SECURITY_GROUP_NAME: &str = "default";

/// The id of the DEFAULT security group of `vpc_id` — the one EC2 creates with the VPC —
/// or `None` when EC2 could not be asked or reports no such group.
///
/// Read with EC2's own filters (`vpc-id` and `group-name`) rather than by listing the
/// account's groups, so the answer is the group EC2 itself calls the default one of that
/// VPC. Nothing is invented for a VPC whose groups are not known: a caller that cannot
/// resolve it attaches no group at all.
pub async fn default_security_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    vpc_id: &str,
) -> Option<String> {
    let in_vpc = ::aws_sdk_ec2::types::Filter::builder()
        .name("vpc-id")
        .values(vpc_id)
        .build();
    let named_default = ::aws_sdk_ec2::types::Filter::builder()
        .name("group-name")
        .values(DEFAULT_SECURITY_GROUP_NAME)
        .build();
    let answer = ec2(ctx, region)
        .describe_security_groups()
        .filters(in_vpc)
        .filters(named_default)
        .send()
        .await;
    match answer {
        Ok(output) => output
            .security_groups()
            .iter()
            .find_map(|group| group.group_id().map(str::to_string)),
        Err(error) => {
            ::tracing::debug!(
                %vpc_id,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the default security group of the VPC could not be resolved against EC2"
            );
            None
        }
    }
}

/// What EC2 says about the instances a request named as targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstanceTargets {
    /// EC2 resolved every id, and reports which VPC each instance is in (when it
    /// carries one).
    Resolved(Vec<(String, Option<String>)>),
    /// EC2 reports that the account holds no instance with this id.
    Absent(String),
    /// Nothing is known: there is no EC2 to ask, or it answered something else.
    Unknown,
}

/// How many instance ids one read of EC2 names. EC2 bounds the ids a describe accepts,
/// so a long target list is resolved in several reads rather than one it would refuse.
const INSTANCE_IDS_PER_READ: usize = 200;

/// What EC2 says about the instance `instance_id` names.
///
/// EC2 refuses a read of an id the account holds no instance for with
/// `InvalidInstanceID.NotFound`, and one that is not an instance id at all with
/// `InvalidInstanceID.Malformed`; both say the same thing about a target — it names no
/// instance of this account — so both answer [`InstanceLookup::Absent`]. Any other
/// failure is this emulator failing to ask rather than EC2 reporting a bad instance, so
/// it says nothing.
///
/// The VPC is the one the instance reports, else the one of the SUBNET it reports: an
/// instance is placed in a subnet, and the subnet is what says which VPC that is.
pub async fn instance_lookup(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    instance_id: &str,
) -> InstanceLookup {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    let answer = ec2(ctx, region)
        .describe_instances()
        .instance_ids(instance_id)
        .send()
        .await;
    let error = match answer {
        Ok(output) => {
            let found = output
                .reservations()
                .iter()
                .flat_map(::aws_sdk_ec2::types::Reservation::instances)
                .find(|instance| instance.instance_id() == Some(instance_id))
                .map(|instance| {
                    (
                        instance.vpc_id().map(str::to_string),
                        instance.subnet_id().map(str::to_string),
                    )
                });
            return match found {
                Some((vpc_id, subnet_id)) => InstanceLookup::Found {
                    vpc_id: match vpc_id {
                        Some(vpc_id) => Some(vpc_id),
                        None => vpc_of_subnets(ctx, region, subnet_id.as_slice()).await,
                    },
                },
                // A read that resolved but reports no such instance says the same thing
                // the refusal below does.
                None => InstanceLookup::Absent,
            };
        }
        Err(error) => error,
    };
    let code = error
        .as_service_error()
        .and_then(ProvideErrorMetadata::code)
        .unwrap_or_default()
        .to_string();
    if code.starts_with("InvalidInstanceID.") {
        return InstanceLookup::Absent;
    }
    ::tracing::debug!(
        %instance_id,
        error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
        "the instance a request named could not be resolved against EC2"
    );
    InstanceLookup::Unknown
}

/// The VPC the subnets EC2 holds for these ids belong to, or `None` when they resolve to
/// none or do not agree on one.
async fn vpc_of_subnets(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[String],
) -> Option<String> {
    let subnets = subnets_by_id(ctx, region, subnet_ids).await?;
    // The one reading of "these subnets agree on one VPC", which the operations that place
    // a load balancer apply to the subnets a request names.
    super::subnets_vpc(Some(&subnets))
}

/// What EC2 says about every instance id a request named, read in as few calls as EC2's
/// own bound on a describe allows.
///
/// A read naming an id the account holds no instance for is refused WHOLE, so it says
/// nothing about the other ids in it; the ids of such a read are then resolved one at a
/// time, and the FIRST that names no instance is the one the refusal names — the caller
/// has to fix that one before any answer about the rest matters.
pub async fn resolve_instance_targets(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    instance_ids: &[String],
) -> InstanceTargets {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    if instance_ids.is_empty() {
        return InstanceTargets::Resolved(Vec::new());
    }
    let mut resolved: Vec<(String, Option<String>)> = Vec::new();
    for chunk in instance_ids.chunks(INSTANCE_IDS_PER_READ) {
        let answer = ec2(ctx, region)
            .describe_instances()
            .set_instance_ids(Some(chunk.to_vec()))
            .send()
            .await;
        let error = match answer {
            Ok(output) => {
                let reported: Vec<(String, Option<String>, Option<String>)> = output
                    .reservations()
                    .iter()
                    .flat_map(::aws_sdk_ec2::types::Reservation::instances)
                    .map(|instance| {
                        (
                            instance.instance_id().unwrap_or_default().to_string(),
                            instance.vpc_id().map(str::to_string),
                            instance.subnet_id().map(str::to_string),
                        )
                    })
                    .collect();
                if let Some(missing) = chunk
                    .iter()
                    .find(|id| !reported.iter().any(|(reported, ..)| reported == *id))
                {
                    return InstanceTargets::Absent(missing.clone());
                }
                // An instance that reports no VPC is placed in a subnet, and the subnet
                // is what says which VPC that is — one read for the whole chunk.
                let subnet_ids: Vec<String> = reported
                    .iter()
                    .filter(|(_, vpc_id, _)| vpc_id.is_none())
                    .filter_map(|(_, _, subnet_id)| subnet_id.clone())
                    .collect();
                let subnets = subnets_by_id(ctx, region, &subnet_ids)
                    .await
                    .unwrap_or_default();
                resolved.extend(reported.into_iter().map(|(id, vpc_id, subnet_id)| {
                    let vpc_id = vpc_id.or_else(|| {
                        let subnet_id = subnet_id?;
                        subnets
                            .iter()
                            .find(|subnet| subnet.subnet_id == subnet_id)?
                            .vpc_id
                            .clone()
                    });
                    (id, vpc_id)
                }));
                continue;
            }
            Err(error) => error,
        };
        let code = error
            .as_service_error()
            .and_then(ProvideErrorMetadata::code)
            .unwrap_or_default()
            .to_string();
        if !code.starts_with("InvalidInstanceID.") {
            ::tracing::debug!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the instances a request named could not be resolved against EC2"
            );
            return InstanceTargets::Unknown;
        }
        // The read was refused on one of its ids. Ask about them one at a time to find
        // which, and stop at the first: that is the one the caller is told about.
        for id in chunk {
            match instance_lookup(ctx, region, id).await {
                InstanceLookup::Absent => return InstanceTargets::Absent(id.clone()),
                InstanceLookup::Found { vpc_id } => resolved.push((id.clone(), vpc_id)),
                InstanceLookup::Unknown => return InstanceTargets::Unknown,
            }
        }
    }
    InstanceTargets::Resolved(resolved)
}

/// The subnet of `subnets` whose CIDR blocks hold `address`, if any.
pub fn containing_subnet(subnets: &[SubnetFacts], address: IpAddr) -> Option<&SubnetFacts> {
    subnets
        .iter()
        .find(|subnet| subnet.cidrs.iter().any(|cidr| cidr.contains(&address)))
}

/// Whether `address` is inside one of `cidrs`.
pub fn inside_any(cidrs: &[IpNet], address: IpAddr) -> bool {
    cidrs.iter().any(|cidr| cidr.contains(&address))
}

/// Whether `address` is one an `ip` target may carry when it is outside the target
/// group's VPC: an address of [`PRIVATE_TARGET_RANGES`]. A publicly routable address is
/// never one.
pub fn is_private_target_address(address: IpAddr) -> bool {
    private_target_ranges()
        .iter()
        .any(|range| range.contains(&address))
}

/// The Availability Zone an `ip` target registration takes, or why the address is not one
/// this target group can carry.
///
/// The rule, from `TargetDescription$AvailabilityZone` — "If the target type is `ip` and
/// the IP address is in a subnet of the VPC for the target group, the Availability Zone is
/// automatically detected and this parameter is optional. If the IP address is outside the
/// VPC, this parameter is required" — together with the addresses `TargetType` allows an
/// `ip` target to come from ("the IP addresses from the subnets of the virtual private
/// cloud (VPC) for the target group, the RFC 1918 range (10.0.0.0/8, 172.16.0.0/12, and
/// 192.168.0.0/16), and the RFC 6598 range (100.64.0.0/10). You can't specify publicly
/// routable IP addresses"):
///
/// * an address in a SUBNET of the target group's VPC is accepted, and takes that
///   subnet's zone whatever the request said;
/// * an address inside the VPC's own CIDR but in none of its subnets is in no zone the
///   load balancer reaches, so it is refused;
/// * an address outside the VPC is accepted only from one of the ranges above — judged
///   first, because whether the address is one an `ip` target may carry at all does not
///   depend on the zone — and then needs the zone that is to serve it.
///
/// So WHICH refusal a registration earns is decided by where the address falls, never by
/// whether the request named a zone, and each refusal states that class's own constraint
/// against the address the request carried ([`address_not_allowed_message`],
/// [`address_in_no_subnet_message`], [`availability_zone_required_message`]).
///
/// `vpc_cidrs` and `subnets` are what EC2 answered for the group's VPC; a caller that
/// could not ask EC2 does not call this at all, because a refusal has to be grounded in
/// what EC2 reported.
pub fn resolve_ip_target(
    address: IpAddr,
    availability_zone: Option<&str>,
    vpc_cidrs: &[IpNet],
    subnets: &[SubnetFacts],
) -> Result<Option<String>, String> {
    if let Some(subnet) = containing_subnet(subnets, address) {
        return Ok(subnet
            .availability_zone
            .clone()
            .or_else(|| availability_zone.map(str::to_string)));
    }
    if inside_any(vpc_cidrs, address) {
        return Err(address_in_no_subnet_message(address));
    }
    // Outside the VPC. Whether the address is one an `ip` target may carry AT ALL does not
    // depend on the zone, so it is answered first: the ranges such a target comes from are
    // what the address is judged against (`TargetType`), whether or not the request named a
    // zone, and naming one would not make an address outside them registrable.
    if !is_private_target_address(address) {
        return Err(address_not_allowed_message(address));
    }
    // The address is one the group may carry, so what is left is which zone serves it:
    // "an Availability Zone or `all`. This determines whether the target receives traffic
    // from the load balancer nodes in the specified Availability Zone or from all enabled
    // Availability Zones for the load balancer" — and nothing else can be inferred, since
    // no subnet of the group's VPC holds it.
    let Some(zone) = availability_zone else {
        return Err(availability_zone_required_message(address));
    };
    Ok(Some(zone.to_string()))
}

/// The address `id` names for an `ip` target group of this `ip_address_type`, or why it is
/// none.
///
/// A target of such a group IS an address (`TargetDescription$Id`: "if the target type is
/// `ip`, specify an IP address"), of the version the group carries its targets in
/// (`TargetGroup$IpAddressType`: "the IP address type. The default value is ipv4") — so a
/// value that is not an address of that version names no target, whatever else is in the
/// request, and nothing outside this request is needed to say so.
pub fn parse_ip_target(id: &str, ip_address_type: Option<&str>) -> Result<IpAddr, String> {
    let ipv6 = ip_address_type == Some("ipv6");
    let version = if ipv6 { "IPv6" } else { "IPv4" };
    match id.parse::<IpAddr>() {
        Ok(IpAddr::V6(address)) if ipv6 => Ok(IpAddr::V6(address)),
        Ok(IpAddr::V4(address)) if !ipv6 => Ok(IpAddr::V4(address)),
        _ => Err(format!("The IP address '{id}' is not a valid {version} address")),
    }
}

/// The refusal an address no `ip` target may carry earns: it is of none of the ranges
/// such a target comes from, so no zone and no VPC makes it registrable. Names the
/// address the request carried and the ranges one must come from.
pub fn address_not_allowed_message(address: IpAddr) -> String {
    format!(
        "The IP address '{address}' is not allowed, as it must be an address from a VPC \
         subnet, RFC1918, or RFC6598"
    )
}

/// The refusal an address inside the target group's VPC but in none of its subnets earns:
/// the load balancer reaches a target through a subnet of that VPC, so there is nothing to
/// supply for an address the VPC holds outside every one of them. Names the address and
/// that constraint — and no VPC id, which the request never named.
pub fn address_in_no_subnet_message(address: IpAddr) -> String {
    format!(
        "The IP address '{address}' is within the VPC, but is not within a VPC subnet so \
         it can't be registered"
    )
}

/// The refusal an `ip` target outside the target group's VPC earns when the request names
/// no Availability Zone for it: the address itself is one the group may carry, so what is
/// missing is the member — the zone that is to serve the address, or `all` for all enabled
/// Availability Zones (`TargetDescription$AvailabilityZone`) — and the sentence names the
/// address so the caller knows which target it has to supply it for.
pub fn availability_zone_required_message(address: IpAddr) -> String {
    format!(
        "You must specify a valid Availability Zone or select 'all' for all enabled \
         Availability zones, since the IP address '{address}' is outside the VPC"
    )
}
