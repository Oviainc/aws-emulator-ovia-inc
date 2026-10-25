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

/// The subnets EC2 holds for the ids a request named, in the order EC2 answered, or
/// `None` when EC2 could not be asked or refused the read (an id that names no subnet of
/// the account refuses the WHOLE read, so nothing is known about any of them).
pub async fn subnets_by_id(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[String],
) -> Option<Vec<SubnetFacts>> {
    if subnet_ids.is_empty() {
        return None;
    }
    let answer = ec2(ctx, region)
        .describe_subnets()
        .set_subnet_ids(Some(subnet_ids.to_vec()))
        .send()
        .await;
    match answer {
        Ok(output) => Some(output.subnets().iter().map(subnet_facts).collect()),
        Err(error) => {
            ::tracing::debug!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the subnets a request named could not be resolved against EC2"
            );
            None
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

/// The VPC each security group a request named belongs to, keyed by group id, or `None`
/// when EC2 could not be asked (including a group id that names no security group of the
/// account, which refuses the whole read).
pub async fn security_group_vpcs(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    group_ids: &[String],
) -> Option<Vec<(String, Option<String>)>> {
    if group_ids.is_empty() {
        return None;
    }
    let answer = ec2(ctx, region)
        .describe_security_groups()
        .set_group_ids(Some(group_ids.to_vec()))
        .send()
        .await;
    match answer {
        Ok(output) => Some(
            output
                .security_groups()
                .iter()
                .map(|group| {
                    (
                        group.group_id().unwrap_or_default().to_string(),
                        group.vpc_id().map(str::to_string),
                    )
                })
                .collect(),
        ),
        Err(error) => {
            ::tracing::debug!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the security groups a request named could not be resolved against EC2"
            );
            None
        }
    }
}

/// Whether any security group a request names belongs to a VPC OTHER than `vpc_id`, as
/// EC2 reported it.
///
/// A load balancer's security groups govern traffic to its network interfaces, which live
/// in the subnets it is placed in: a group of another VPC can never be applied to them.
/// `false` when there is no EC2 to ask, because a refusal has to be grounded in what EC2
/// reported rather than assumed from silence — and `false` for a request that names no
/// group at all, which associates nothing foreign.
///
/// The ONE reading of that rule, called by `CreateLoadBalancer` for the groups a new load
/// balancer starts with and by `SetSecurityGroups` for the groups that replace them.
pub async fn has_foreign_security_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    vpc_id: &str,
    security_groups: &[String],
) -> bool {
    let Some(reported) = security_group_vpcs(ctx, region, security_groups).await else {
        return false;
    };
    reported.iter().any(|(_, group_vpc)| {
        group_vpc
            .as_deref()
            .is_some_and(|holding| holding != vpc_id)
    })
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
    let mut vpcs = subnets.iter().filter_map(|subnet| subnet.vpc_id.as_deref());
    let first = vpcs.next()?;
    vpcs.all(|vpc| vpc == first).then(|| first.to_string())
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
/// `vpc_cidrs` and `subnets` are what EC2 answered for the group's VPC; a caller that
/// could not ask EC2 does not call this at all, because a refusal has to be grounded in
/// what EC2 reported.
pub fn resolve_ip_target(
    address: IpAddr,
    availability_zone: Option<&str>,
    vpc_id: &str,
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
        return Err(format!(
            "The IP address '{address}' is in the CIDR of VPC {vpc_id} but is not in any \
             of its subnets"
        ));
    }
    // Outside the VPC. Whether the address is one an `ip` target may carry AT ALL does not
    // depend on the zone, so it is answered first: a publicly routable address is refused
    // on its own terms ("you can't specify publicly routable IP addresses"), whether or
    // not the request named a zone, and naming one would not make it registrable.
    if !is_private_target_address(address) {
        return Err(format!(
            "The IP address '{address}' is not valid: you can't specify publicly routable \
             IP addresses"
        ));
    }
    // The address is one the group may carry, so what is left is which zone serves it:
    // "an Availability Zone or `all`. This determines whether the target receives traffic
    // from the load balancer nodes in the specified Availability Zone or from all enabled
    // Availability Zones for the load balancer" — and nothing else can be inferred, since
    // no subnet of the group's VPC holds it.
    let Some(zone) = availability_zone else {
        return Err(AVAILABILITY_ZONE_REQUIRED.to_string());
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

/// The refusal an `ip` target outside the target group's VPC earns when the request names
/// no Availability Zone for it: a fixed sentence, because what the caller has to supply is
/// the member rather than anything about the value — the zone that is to serve the
/// address, or `all` for "all enabled Availability Zones for the load balancer"
/// (`TargetDescription$AvailabilityZone`).
pub const AVAILABILITY_ZONE_REQUIRED: &str =
    "You must specify a valid Availability Zone or select 'all' for all enabled \
     Availability Zones for the load balancer";
