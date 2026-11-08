// SPDX-License-Identifier: CC0-1.0

//! The subnets a cluster subnet group is made of, as the service that OWNS them reports
//! them.
//!
//! A subnet is not a resource of this service: it is Amazon EC2's. A client builds its
//! network with `ec2:CreateVpc`/`ec2:CreateSubnet`, or uses the default VPC EC2 materialises
//! for every Region of the account, and then names those subnets in
//! `CreateClusterSubnetGroup$SubnetIds`. So EC2 is the ONE answer to "which subnets does the
//! account have", and an identifier a request names is resolved there and nowhere else —
//! reached the way any emulated service reaches another, through the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so this crate names no other
//! service's crate and nothing leaves the process.
//!
//! What a subnet group reports about its network — its `VpcId` and each subnet's
//! `SubnetAvailabilityZone` — is read off the records EC2 answered with, never built from
//! the identifiers or the Region name: a client that cross-checks a group against
//! `ec2:DescribeSubnets` must see one answer, not two.
//!
//! A hop that could not be made at all (no EC2 in this process, an answer that could not be
//! read) is evidence of nothing, and this service PLACES the group in those subnets and
//! reports their VPC and zones — so with no record there is nothing to place it in or to
//! report, and the create is refused rather than stored against a network nobody could
//! confirm (`services/common/src/network.rs` states which reading belongs to which kind of
//! service).
//!
//! Nothing is cached: a subnet created a moment ago resolves at once, and one that was
//! deleted stops resolving at once.

use std::collections::{HashMap, HashSet};

use super::{default_subnet_group, DEFAULT_CLUSTER_SUBNET_GROUP};

/// The `DescribeSubnets` filter that selects subnets BY ID (`subnet-id`, listed by that
/// operation's own `Filters` documentation).
///
/// A filter is what asks the question rather than `SubnetIds`: an identifier the account has
/// no subnet for is then an absent row instead of a failure of the whole call, so every
/// identifier the request named is still resolved against what EC2 does hold.
const SUBNET_ID_FILTER: &str = "subnet-id";

/// The `DescribeSubnets` filter that selects the subnets OF ONE VPC (`vpc-id`).
const VPC_ID_FILTER: &str = "vpc-id";

/// The `DescribeVpcs` filter that selects the Region's DEFAULT VPC (`is-default`), and the
/// value that selects it.
const IS_DEFAULT_FILTER: &str = "is-default";
const IS_DEFAULT_VALUE: &str = "true";

/// The most subnet identifiers ONE hop asks about, as values of one `subnet-id` filter.
///
/// The owner evaluates a filter against every subnet the account stores, so the cost of a
/// hop is (identifiers asked x subnets stored): this caps the first factor, so one hop costs
/// a constant times what the account stores rather than something the request chooses. A
/// group holds at most [`super::SUBNETS_PER_SUBNET_GROUP_QUOTA`] subnets — checked before
/// anything is resolved — so a create fits in one hop.
const SUBNET_IDS_PER_HOP: usize = 64;

/// One subnet of the account, exactly as EC2 reports it: nothing here is derived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSubnet {
    /// `Subnet$SubnetId` — the identifier EC2 minted.
    pub subnet_id: String,
    /// `Subnet$VpcId` — the VPC that CONTAINS this subnet, which is the VPC a group made of
    /// it reports.
    pub vpc_id: String,
    /// `Subnet$AvailabilityZone` — the zone this subnet lies in, which is the zone the
    /// group reports for it.
    pub availability_zone: String,
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
/// (`service_common::arn::region_label`, the one place that grammar is stated) instead of
/// being interpolated as given: `None` for a value that would name an authority the caller
/// chose rather than the peer service this emulator asks.
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

/// One `ec2:DescribeSubnets` hop, filtered by `name` = `values`, as [`ResolvedSubnet`]
/// records in the order EC2 answered.
///
/// A subnet EC2 reports without an id, a VPC or a zone is dropped: those three are what this
/// service reports about a subnet, and nothing is invented for a record missing one.
async fn described_subnets(
    client: &::aws_sdk_ec2::Client,
    region: &str,
    name: &str,
    values: Vec<String>,
) -> Option<Vec<ResolvedSubnet>> {
    let filter = ::aws_sdk_ec2::types::Filter::builder()
        .name(name)
        .set_values(Some(values))
        .build();
    match client.describe_subnets().filters(filter).send().await {
        Ok(output) => Some(
            output
                .subnets()
                .iter()
                .filter_map(|subnet| {
                    Some(ResolvedSubnet {
                        subnet_id: subnet.subnet_id()?.to_string(),
                        vpc_id: subnet.vpc_id()?.to_string(),
                        availability_zone: subnet.availability_zone()?.to_string(),
                    })
                })
                .collect(),
        ),
        Err(error) => {
            // At `warn`: a subnet group cannot be placed when the owner of subnets cannot be
            // asked, so a deployment where this hop fails is one where no group can be
            // created at all — worth seeing, and it IS why nothing resolved.
            ::tracing::warn!(
                %region,
                filter = %name,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the subnets a cluster subnet group names could not be resolved against EC2"
            );
            None
        }
    }
}

/// Every subnet of `subnet_ids` the account has in `region`, by identifier, as EC2 reports
/// it — an identifier it holds no subnet for is simply absent from the map — or `None` where
/// EC2 could not be ASKED at all, which is evidence of nothing.
///
/// Bounded by [`SUBNET_IDS_PER_HOP`] identifiers per hop.
async fn held_subnets(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[&str],
) -> Option<HashMap<String, ResolvedSubnet>> {
    let mut held: HashMap<String, ResolvedSubnet> = HashMap::new();
    let Some(client) = ec2(ctx, region) else {
        ::tracing::warn!("the request names no Region label, so EC2 cannot be asked");
        return None;
    };
    for chunk in subnet_ids.chunks(SUBNET_IDS_PER_HOP) {
        let asked = chunk.iter().map(|id| (*id).to_string()).collect();
        let answered = described_subnets(&client, region, SUBNET_ID_FILTER, asked).await?;
        for subnet in answered {
            held.insert(subnet.subnet_id.clone(), subnet);
        }
    }
    Some(held)
}

/// What asking the owner of subnets about the identifiers a request named answered.
#[derive(Debug, Clone)]
pub enum SubnetResolution {
    /// Every identifier named a subnet of the account: one record each, in the order the
    /// request listed them.
    Resolved(Vec<ResolvedSubnet>),
    /// EC2 answered, and these identifiers — in request order, without repeats — name no
    /// subnet of the account. The request is what is wrong, and this is what the refusal
    /// names (see [`invalid_subnets_message`]).
    Unresolved(Vec<String>),
    /// EC2 could not be ASKED at all (it is not mounted in this process, or the answer could
    /// not be read), so nothing is known about any of the identifiers. That is not the
    /// caller's doing, and this service has no record to place the group in or to report:
    /// the caller answers the modelled "a dependent service is unavailable" fault rather
    /// than blaming the request.
    Unavailable,
}

/// Every subnet identifier a request names, resolved against the subnets the account has in
/// `region`, in the order the request lists them.
///
/// Identifiers are deduplicated before they are asked about, so the work does not grow with
/// how many times one subnet is named. See [`SubnetResolution`] for the three answers.
pub async fn resolve_requested_subnets(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[String],
) -> SubnetResolution {
    let mut distinct: Vec<&str> = Vec::with_capacity(subnet_ids.len());
    let mut seen: HashSet<&str> = HashSet::new();
    for subnet_id in subnet_ids {
        if seen.insert(subnet_id.as_str()) {
            distinct.push(subnet_id.as_str());
        }
    }
    let Some(held) = held_subnets(ctx, region, &distinct).await else {
        return SubnetResolution::Unavailable;
    };

    let unresolved: Vec<String> = distinct
        .iter()
        .filter(|subnet_id| !held.contains_key(**subnet_id))
        .map(|subnet_id| (*subnet_id).to_string())
        .collect();
    if !unresolved.is_empty() {
        return SubnetResolution::Unresolved(unresolved);
    }
    SubnetResolution::Resolved(
        subnet_ids
            .iter()
            .filter_map(|subnet_id| held.get(subnet_id).cloned())
            .collect(),
    )
}

/// The subnets of the Region's DEFAULT VPC, as EC2 reports them, or an empty list where the
/// account has no default VPC there — or where EC2 could not be asked at all.
///
/// Asked in two hops rather than guessed: which VPC is the default one
/// (`ec2:DescribeVpcs`, `is-default`) and then which subnets belong to it
/// (`ec2:DescribeSubnets`, `vpc-id`). That is what makes the account's ambient subnet group
/// cover the network the account really has.
pub async fn default_vpc_subnets(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Vec<ResolvedSubnet> {
    let Some(client) = ec2(ctx, region) else {
        ::tracing::warn!("the request names no Region label, so EC2 cannot be asked");
        return Vec::new();
    };
    let default_vpc = match client
        .describe_vpcs()
        .filters(
            ::aws_sdk_ec2::types::Filter::builder()
                .name(IS_DEFAULT_FILTER)
                .values(IS_DEFAULT_VALUE)
                .build(),
        )
        .send()
        .await
    {
        Ok(output) => output
            .vpcs()
            .iter()
            .find_map(|vpc| vpc.vpc_id().map(str::to_string)),
        Err(error) => {
            ::tracing::warn!(
                %region,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the account's default VPC could not be resolved against EC2"
            );
            None
        }
    };
    let Some(default_vpc) = default_vpc else {
        return Vec::new();
    };
    described_subnets(&client, region, VPC_ID_FILTER, vec![default_vpc])
        .await
        .unwrap_or_default()
}

/// Materialise the account's DEFAULT cluster subnet group, over the default VPC's own
/// subnets, unless this store already holds it.
///
/// An account HAS this group before anybody creates one (see
/// [`DEFAULT_CLUSTER_SUBNET_GROUP`]), so every read must find it and a cluster that names no
/// subnet group is launched into it. Writing it here is idempotent and materialises a record
/// owed since the account existed rather than one this call created, so it changes nothing a
/// later caller would not have seen anyway.
///
/// The EC2 hop happens with NO lock held, and a Region whose default VPC EC2 reports nothing
/// for leaves the store untouched: a group over subnets nobody could confirm is not one this
/// service reports.
pub async fn materialize_default_subnet_group<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &Eng,
    region: &str,
) {
    let Some(state) = super::service_state(engine) else {
        return;
    };
    if state
        .lock()
        .subnet_group(DEFAULT_CLUSTER_SUBNET_GROUP)
        .is_some()
    {
        return;
    }
    let subnets = default_vpc_subnets(ctx, region).await;
    if subnets.is_empty() {
        ::tracing::warn!(
            %region,
            "the account's default VPC has no subnets EC2 answered for, so it has no default \
             cluster subnet group here"
        );
        return;
    }
    let mut store = state.lock();
    // Checked again under the lock: another request may have materialised it while this one
    // was asking EC2, and the group is one record whoever gets there first.
    if store.subnet_group(DEFAULT_CLUSTER_SUBNET_GROUP).is_none() {
        store.subnet_groups.push(default_subnet_group(&subnets));
    }
}

/// How a create refuses a `SubnetIds` list naming subnets the account does not have:
/// `Some input subnets in :[<ids>] are invalid.`
///
/// Recorded Amazon Redshift wording, with the offending identifiers — those that resolved to
/// no subnet of the account, in the order the request listed them — comma-joined inside the
/// brackets.
pub fn invalid_subnets_message(subnet_ids: &[String]) -> String {
    format!(
        "Some input subnets in :[{}] are invalid.",
        subnet_ids.join(",")
    )
}

/// Why a create or a modify cannot be served at all when the owner of subnets could not be
/// ASKED about the identifiers the request named.
///
/// `smithy_query(query="shape", shape="DependentServiceUnavailableFault")` — "Your request
/// cannot be completed because a dependent internal service is temporarily unavailable. Wait
/// 30 to 60 seconds and try again." The fault's own documented sentence is the fixed text:
/// nothing about the REQUEST is wrong, so nothing about it is said.
pub const SUBNETS_UNRESOLVABLE: &str =
    "Your request cannot be completed because a dependent internal service is temporarily \
     unavailable. Wait 30 to 60 seconds and try again.";
