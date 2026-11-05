// SPDX-License-Identifier: CC0-1.0

//! The network a VPC endpoint is placed in: facts this service RESOLVES rather than
//! holds.
//!
//! `VPCOptions` names the subnets and security groups an endpoint is created in, and
//! `VPCDerivedInfo` reports back the two members the model says are DERIVED from those
//! subnets: `VPCId` — "the ID for your VPC. Amazon VPC generates this value when you
//! create a VPC" — and `AvailabilityZones`, "the list of Availability Zones associated
//! with the VPC subnets". Neither is a resource of this service: a subnet is Amazon
//! EC2's, so which VPC and which zone one is in is EC2's to say, and what a request
//! names is resolved against EC2 — reached the way any emulated service reaches another,
//! through the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so this crate names no other
//! service's crate.
//!
//! Everything built here is grounded in what EC2 ANSWERED: a subnet it reports nothing
//! for is left unresolved, and an emulator with no EC2 mounted knows nothing about the
//! account's network, so nothing is what it refuses on — a refusal is never assumed from
//! silence. That is the reading a service that only GATES on the network takes, as
//! `service_common::network` records; this service keeps what EC2 answered as well,
//! because `VPCDerivedInfo` reports it, and keeps it only for the subnets EC2 resolved.

use std::collections::BTreeSet;

use super::*;

/// The EC2 filter that selects subnets BY ID (`subnet-id`, listed by `DescribeSubnets`'s
/// own `Filters` documentation).
///
/// A filter rather than `SubnetIds`: an identifier the account has no subnet for is then
/// an absent row instead of a failure of the whole call, so every identifier the request
/// named is still resolved against what EC2 does hold.
const SUBNET_ID_FILTER: &str = "subnet-id";

/// The most subnet identifiers one call resolves, in ONE hop.
///
/// CHOSEN as a process-safety ceiling, not a published quota: the model declares no
/// `@length` on `VPCOptions$SubnetIds`, so without one the work a single request causes
/// would be sized by that request. A VPC endpoint is placed with one subnet per
/// Availability Zone, so this is far above what a real placement names; identifiers past
/// it are left unresolved, which — like an EC2 that cannot be asked — grounds nothing.
const SUBNET_IDS_PER_CALL: usize = 64;

/// One subnet of the account, reduced to what this service reads off it: which VPC it
/// belongs to and which Availability Zone it sits in, as EC2 reports them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubnetPlacement {
    /// The identifier the request named, which is the one EC2 answered for.
    pub subnet_id: String,
    /// The VPC EC2 reports the subnet in, absent where its record carries none.
    pub vpc_id: Option<String>,
    /// The Availability Zone EC2 reports the subnet in, absent where its record carries
    /// none.
    pub availability_zone: Option<String>,
}

/// An AWS SDK client for EC2 over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated service:
/// it routes in process, so no network client is configured here and nothing leaves this
/// process. The credentials are the callback's, which sign this in-process hop and never
/// appear in a response.
///
/// The Region a request is signed for is CALLER-SUPPLIED text and here it ends up in an
/// endpoint URL's authority, so it is read as a Region LABEL
/// (`service_common::arn::region_label`, the one place this tree states that grammar)
/// rather than interpolated as given: `None` for a value that would name an authority the
/// caller chose instead of the peer service this emulator asks.
fn ec2(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Option<::aws_sdk_ec2::Client> {
    let Some(region) = ::service_common::arn::region_label(region) else {
        ::tracing::warn!("the request names no Region label, so EC2 cannot be asked");
        return None;
    };
    let config = ::aws_sdk_ec2::Config::builder()
        .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_ec2::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://ec2.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for
        // a stalled stream, and each of those would need a sleep implementation this
        // client is not given. The same goes for caching the identity: the credentials
        // the callback hands over are used directly.
        .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled())
        .build();
    Some(::aws_sdk_ec2::Client::from_conf(config))
}

/// Every subnet of `subnet_ids` the account has in `region`, as EC2 reports it, in the
/// order the request named them.
///
/// An identifier EC2 reports no subnet for is simply ABSENT from the answer, and so is
/// every identifier when there is no EC2 to ask: this resolves what the account's network
/// says, and says nothing of its own about what it could not resolve.
///
/// Bounded by [`SUBNET_IDS_PER_CALL`] identifiers, asked about in one hop, deduplicated —
/// so the work does not grow with how many times one subnet is named.
pub async fn resolve_subnets(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[String],
) -> Vec<SubnetPlacement> {
    let mut asked: Vec<&String> = Vec::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for subnet_id in subnet_ids {
        if asked.len() == SUBNET_IDS_PER_CALL {
            break;
        }
        if seen.insert(subnet_id.as_str()) {
            asked.push(subnet_id);
        }
    }
    if asked.is_empty() {
        return Vec::new();
    }
    let Some(client) = ec2(ctx, region) else {
        return Vec::new();
    };
    let by_id = ::aws_sdk_ec2::types::Filter::builder()
        .name(SUBNET_ID_FILTER)
        .set_values(Some(asked.iter().map(|id| (*id).clone()).collect()))
        .build();
    let reported = match client.describe_subnets().filters(by_id).send().await {
        Ok(output) => output.subnets().to_vec(),
        Err(error) => {
            // At `warn`: a deployment where this hop fails is one where the network
            // behind an endpoint is never resolved, which is worth seeing — and it IS
            // why an identifier did not resolve.
            ::tracing::warn!(
                %region,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the subnets a request named could not be resolved against EC2"
            );
            return Vec::new();
        }
    };
    // In the order the REQUEST named them, so what a caller is told about is the subnet
    // it listed rather than whichever row EC2 happened to answer first.
    subnet_ids
        .iter()
        .filter_map(|subnet_id| {
            let subnet = reported
                .iter()
                .find(|subnet| subnet.subnet_id() == Some(subnet_id.as_str()))?;
            Some(SubnetPlacement {
                subnet_id: subnet_id.clone(),
                vpc_id: subnet.vpc_id().map(str::to_owned),
                availability_zone: subnet.availability_zone().map(str::to_owned),
            })
        })
        .collect()
}

/// The ONE VPC every resolved subnet belongs to, or `None` where they resolve to none or
/// to more than one.
///
/// `VPCDerivedInfo$VPCId` is a single VPC, so a set of subnets that does not agree on one
/// names no VPC to report — which is the same answer as a placement nothing could be
/// resolved for.
pub fn placed_vpc(placements: &[SubnetPlacement]) -> Option<String> {
    let mut vpcs = placements.iter().filter_map(|placement| placement.vpc_id.as_deref());
    let first = vpcs.next()?;
    vpcs.all(|vpc| vpc == first).then(|| first.to_owned())
}

/// The Availability Zones the resolved subnets sit in, in the order the request named the
/// subnets, each zone once; `None` where no resolved subnet names a zone.
pub fn placed_zones(placements: &[SubnetPlacement]) -> Option<Vec<String>> {
    let mut zones: Vec<String> = Vec::new();
    for zone in placements
        .iter()
        .filter_map(|placement| placement.availability_zone.as_deref())
    {
        if !zones.iter().any(|seen| seen == zone) {
            zones.push(zone.to_owned());
        }
    }
    (!zones.is_empty()).then_some(zones)
}

/// The first resolved subnet that is NOT in `vpc_id`, in the order the request named
/// them, or `None` where every resolved subnet is in that VPC.
///
/// A subnet EC2 resolved no VPC for is not evidence of being outside one, so it is not
/// what this answers — only a subnet EC2 placed in ANOTHER VPC is.
pub fn subnet_outside_vpc<'a>(
    vpc_id: &str,
    placements: &'a [SubnetPlacement],
) -> Option<&'a SubnetPlacement> {
    placements.iter().find(|placement| {
        placement
            .vpc_id
            .as_deref()
            .is_some_and(|placed| placed != vpc_id)
    })
}

/// `VPCOptions` as the service reports it back, with the two members EC2 derives from the
/// subnets filled in from what EC2 answered for them.
///
/// [`stored_vpc_options`] is what stores the members the REQUEST carried; this adds the
/// derived ones, and adds nothing where the subnets resolved to nothing.
pub fn placed_vpc_options(input: VPCOptions, placements: &[SubnetPlacement]) -> VPCDerivedInfo {
    let mut options = stored_vpc_options(input);
    options.vpc_id = placed_vpc(placements);
    options.availability_zones = placed_zones(placements);
    options
}
