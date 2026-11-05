// SPDX-License-Identifier: CC0-1.0

//! The subnets the ACCOUNT has, as the service that owns them reports them.
//!
//! A subnet is not a resource of this service: it is Amazon EC2's. A client builds its
//! network with `ec2:CreateVpc`/`ec2:CreateSubnet`, or uses the default VPC EC2 materialises
//! for every Region of the account, and then names those subnets in
//! `CreateFileSystem$SubnetIds`. So EC2 is the ONE answer to "which subnets does the account
//! have", and an identifier a request names is resolved there and nowhere else — reached the
//! way any emulated service reaches another, through the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so this crate names no other
//! service's crate and nothing leaves the process.
//!
//! The VPC and the Availability Zone a file system reports come from the record EC2 answered
//! with, never from anything built here, so the placement this service reports is the
//! network the account really has. An identifier EC2 reports no subnet for resolves to
//! nothing, and that is the only case the modelled `InvalidNetworkSettings` answers.
//!
//! Nothing is cached: a subnet created a moment ago resolves at once, and one that was
//! deleted stops resolving at once.

use std::collections::{HashMap, HashSet};

use super::SubnetRecord;

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
/// a constant times what the account stores rather than something the request chooses. The
/// modelled `SubnetIds` list is `length 0..50`, so a create normally fits in one hop.
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

/// Every subnet of `subnet_ids` the account has in `region`, by identifier, as EC2 reports
/// it — an identifier it holds no subnet for is simply absent from the map.
///
/// FAIL-CLOSED: a hop that could not be made at all contributes nothing, so the identifiers
/// it would have answered for resolve to nothing and the create is refused. Nothing about a
/// subnet is invented here, and a subnet that cannot be shown to be the account's is not one
/// a file system is placed in — this operation PLACES the file system in those subnets and
/// reports their VPC, so with no record there is nothing to place it in or to report. (A
/// service that only GATES on a subnet existing, keeping nothing of it, refuses nothing on
/// silence instead; `services/common/src/network.rs` states which reading belongs to which.)
///
/// Bounded by [`SUBNET_IDS_PER_HOP`] identifiers per hop.
async fn ec2_subnets(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[&str],
) -> HashMap<String, SubnetRecord> {
    let mut held: HashMap<String, SubnetRecord> = HashMap::new();
    let Some(client) = ec2(ctx, region) else {
        ::tracing::warn!("the request names no Region label, so EC2 cannot be asked");
        return held;
    };
    for chunk in subnet_ids.chunks(SUBNET_IDS_PER_HOP) {
        let by_id = ::aws_sdk_ec2::types::Filter::builder()
            .name(SUBNET_ID_FILTER)
            .set_values(Some(chunk.iter().map(|id| (*id).to_string()).collect()))
            .build();
        match client.describe_subnets().filters(by_id).send().await {
            Ok(output) => held.extend(output.subnets().iter().filter_map(|subnet| {
                let subnet_id = subnet.subnet_id()?.to_string();
                Some((
                    subnet_id.clone(),
                    SubnetRecord {
                        subnet_id,
                        // EC2 reports the VPC a subnet belongs to and the zone it sits in;
                        // a record missing either carries what this service can report of
                        // it, and nothing is invented for it here.
                        vpc_id: subnet.vpc_id().unwrap_or_default().to_string(),
                        availability_zone: subnet
                            .availability_zone()
                            .unwrap_or_default()
                            .to_string(),
                    },
                ))
            })),
            Err(error) => {
                // At `warn`: a create cannot be placed when the owner of subnets cannot be
                // asked, so a deployment where this hop fails is one where no file system
                // can be created at all — worth seeing, and it IS why an identifier did not
                // resolve.
                ::tracing::warn!(
                    %region,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the subnets a create named could not be resolved against EC2"
                );
            }
        }
    }
    held
}

/// Every subnet identifier a create request names, resolved against the subnets the account
/// has in `region`.
///
/// A file system is placed in the subnets the request names, so an identifier that names no
/// subnet of the account cannot be placed at all: the caller answers the modelled
/// `InvalidNetworkSettings`, naming the identifier in `InvalidSubnetId`, and stores NOTHING.
/// `Err` carries the FIRST such identifier, in the order the request lists them, so the
/// answer does not depend on how many of them are wrong.
///
/// ONE source — the service that owns subnets (see this module's own documentation) — and one
/// hop, asking about every identifier the request named at once, deduplicated, so the work
/// does not grow with how many times one subnet is named.
///
/// BOUNDED BY THE MODEL, not by the request: the modelled `SubnetIds` is `length 0..50` and
/// the per-type `PreferredSubnetId` adds at most one more, and that bound is enforced before
/// this handler runs (`crate::service`'s refusal layer), so the identifiers asked about fit
/// in one hop of at most [`SUBNET_IDS_PER_HOP`] — a constant — whatever the request body
/// carries.
pub async fn resolve_requested_subnets(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[&str],
) -> Result<Vec<SubnetRecord>, String> {
    let mut distinct: Vec<&str> = Vec::with_capacity(subnet_ids.len());
    let mut seen: HashSet<&str> = HashSet::new();
    for subnet_id in subnet_ids {
        if seen.insert(subnet_id) {
            distinct.push(subnet_id);
        }
    }
    let held = ec2_subnets(ctx, region, &distinct).await;

    let mut resolved = Vec::with_capacity(subnet_ids.len());
    for subnet_id in subnet_ids {
        match held.get(*subnet_id) {
            Some(subnet) => resolved.push(subnet.clone()),
            None => return Err((*subnet_id).to_string()),
        }
    }
    Ok(resolved)
}
