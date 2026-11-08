// SPDX-License-Identifier: CC0-1.0

//! WHERE A DIRECTORY'S SERVERS SIT ON THE NETWORK: the subnets a create places them in, and
//! the address each one answers DNS at.
//!
//! `DirectoryVpcSettings$SubnetIds`' own documentation says "Directory Service creates a
//! directory server and a DNS server in each of these subnets", and
//! `DirectoryDescription$DnsIpAddrs` reports "the IP addresses of the DNS servers for the
//! directory" — for a Simple AD or a Managed Microsoft AD directory, the addresses of those
//! very servers. A server placed in a subnet holds an address OUT OF THAT SUBNET'S OWN CIDR
//! block, so the member cannot be answered from the request alone: the request names subnet
//! IDENTIFIERS, and which block each one carries is a fact of the account's network.
//!
//! A subnet is not a resource of this service: it is Amazon EC2's. So the ONE answer to what
//! block a subnet carries, which zone it sits in and whether the account has it at all is
//! EC2 — asked the way any emulated service reaches another, over the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so this crate names no other
//! service's crate and nothing leaves the process (`services/common/src/network.rs` states
//! that contract).
//!
//! What a create keeps of the answer is [`SubnetNetwork`], one per subnet it could resolve,
//! and every address this service reports is counted out of those blocks
//! ([`server_address`]) — so the addresses a directory serves DNS at sit inside the subnets
//! the caller chose, and the directory's controllers, its Availability Zones and its DNS
//! addresses all come from the one record.
//!
//! A hop that could NOT be made is evidence of nothing, and this one GATES no create: a
//! directory is stored with the `VpcSettings` the request named whatever EC2 says, because
//! this service reports that placement back as the request spelled it. An identifier EC2
//! reports no subnet for therefore simply contributes no block, and the directory serves no
//! address for it rather than one invented here.

use std::collections::BTreeMap;

/// One subnet of a directory's placement, as Amazon EC2 — the service that owns subnets —
/// reported it to the create that placed the directory's servers.
///
/// Only what this service REPORTS about the placement is kept: the block the servers'
/// addresses are counted out of and the zone the subnet sits in, which
/// `DirectoryVpcSettingsDescription$AvailabilityZones` ("the list of Availability Zones that
/// the directory is in") and `DomainController$AvailabilityZone` are. Nothing here is
/// derived from the Region name or from the request.
#[derive(Debug, Clone)]
pub struct SubnetNetwork {
    /// The subnet's own identifier, as EC2 reports it — the same identifier the create's
    /// settings block named it by.
    pub subnet_id: String,
    /// The subnet's IPv4 CIDR block, as EC2 reports it; `None` for a subnet it reports none
    /// for, whose servers then hold no IPv4 address.
    pub cidr_block: Option<String>,
    /// The Availability Zone the subnet sits in, as EC2 reports it.
    pub availability_zone: Option<String>,
}

/// How many addresses at the START of a subnet's CIDR block carry no server of the account.
///
/// From AWS's own addressing rules for a VPC subnet: the first four addresses and the last
/// address of every subnet CIDR block are reserved, so the first address a resource of the
/// account can hold is the fifth one in the block.
const RESERVED_LEADING_ADDRESSES: u32 = 4;

/// The address the `ordinal`-th server a directory places in `cidr_block` holds: the
/// `ordinal`-th assignable address of that block.
///
/// THE one place this service turns a subnet's block into a server's address, so a
/// directory's DNS addresses and its domain controllers' addresses cannot disagree, and
/// every address reported sits inside the subnet the server was placed in. Counted from the
/// first assignable address of the block ([`RESERVED_LEADING_ADDRESSES`]), so two servers of
/// one directory in one subnet never share an address.
///
/// `None` when `cidr_block` is not an IPv4 CIDR block, or when the block has no `ordinal`-th
/// assignable address — a block too small to carry that many servers.
#[must_use]
pub fn server_address(cidr_block: &str, ordinal: u32) -> Option<String> {
    let block: ::ipnet::Ipv4Net = cidr_block.parse().ok()?;
    let first = u32::from(block.network()).checked_add(RESERVED_LEADING_ADDRESSES)?;
    let address = first.checked_add(ordinal)?;
    // The last address of the block is reserved too, so an address that reaches it is past
    // what the block can carry.
    if address >= u32::from(block.broadcast()) {
        return None;
    }
    Some(::std::net::Ipv4Addr::from(address).to_string())
}

/// The `DescribeSubnets` filter that selects subnets BY ID (`subnet-id`, listed by that
/// operation's own `Filters` documentation).
///
/// A filter rather than `SubnetIds`: an identifier the account has no subnet for is then an
/// absent row instead of a failure of the whole call, so every identifier the create named
/// is still resolved against what EC2 does hold.
const SUBNET_ID_FILTER: &str = "subnet-id";

/// The most subnet identifiers ONE hop asks about, as values of one `subnet-id` filter —
/// and the most this resolution asks about at all.
///
/// The owner evaluates a filter against every subnet the account stores, so the cost of a hop
/// is (identifiers asked x subnets stored): capping the first factor is what keeps this
/// resolution a CONSTANT times what the account stores rather than something the request
/// chooses. The modelled `SubnetIds` carries no length bound, and a directory's servers sit
/// in [`super::REQUIRED_SUBNETS`] subnets, so a settings block naming more than this is
/// naming far more than any placement can use and the surplus resolves to nothing.
const SUBNET_IDS_PER_HOP: usize = 64;

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
fn ec2(
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

/// The network of every subnet of `subnet_ids` the account has in `region`, keyed by
/// identifier, as EC2 reports it — an identifier it holds no subnet for is simply absent
/// from the map, and so is every identifier when the hop could not be made at all.
///
/// Called by a create BEFORE it takes this service's store, so no guard is held across the
/// hop. ONE hop of at most [`SUBNET_IDS_PER_HOP`] identifiers, whatever the request body
/// carries: nothing caller-sized is asked about, allocated or stored here.
pub async fn resolve_subnet_networks(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[String],
) -> BTreeMap<String, SubnetNetwork> {
    let mut resolved: BTreeMap<String, SubnetNetwork> = BTreeMap::new();
    let Some(client) = ec2(ctx, region) else {
        ::tracing::warn!("the request names no Region label, so EC2 cannot be asked");
        return resolved;
    };
    // Asked for once each, and at most a hop's worth: an identifier the settings block
    // repeats costs no second value in the filter, and a block longer than one hop is
    // resolved as far as that hop reaches.
    let mut distinct: Vec<&str> = Vec::with_capacity(SUBNET_IDS_PER_HOP);
    for subnet_id in subnet_ids {
        if distinct.len() >= SUBNET_IDS_PER_HOP {
            break;
        }
        if !distinct.contains(&subnet_id.as_str()) {
            distinct.push(subnet_id);
        }
    }
    let by_id = ::aws_sdk_ec2::types::Filter::builder()
        .name(SUBNET_ID_FILTER)
        .set_values(Some(distinct.iter().map(|id| (*id).to_string()).collect()))
        .build();
    match client.describe_subnets().filters(by_id).send().await {
        Ok(output) => {
            for subnet in output.subnets() {
                let Some(subnet_id) = subnet.subnet_id() else {
                    continue;
                };
                resolved.insert(
                    subnet_id.to_string(),
                    SubnetNetwork {
                        subnet_id: subnet_id.to_string(),
                        cidr_block: subnet.cidr_block().map(str::to_string),
                        availability_zone: subnet.availability_zone().map(str::to_string),
                    },
                );
            }
        }
        Err(error) => {
            // At `warn`: a directory created where the owner of subnets cannot be asked
            // serves no DNS address, which is worth seeing, and this IS why.
            ::tracing::warn!(
                %region,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the subnets a directory is placed in could not be resolved against EC2"
            );
        }
    }
    resolved
}
