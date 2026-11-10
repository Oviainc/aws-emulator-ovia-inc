// SPDX-License-Identifier: CC0-1.0

//! The NETWORK a managed endpoint's gateway runs in: a fact this service resolves rather
//! than holds.
//!
//! `Endpoint$securityGroup` ("The security group configuration of the endpoint") and
//! `Endpoint$subnetIds` ("The subnet IDs of the endpoint") are not resources of this
//! service. A managed endpoint is a gateway into the CONTAINER CLUSTER the virtual cluster
//! is backed by (`ContainerProvider$id`, "The ID of the container cluster"), and that
//! cluster's networking — the subnets its control plane reaches the account's VPC through
//! and the security group Amazon EKS creates for it — belongs to Amazon EKS, which in turn
//! holds them against Amazon EC2's own subnets and groups.
//!
//! So Amazon EKS is the ONE answer to "which subnets and which group is this container
//! cluster in", and it is ASKED — `eks:DescribeCluster`, in the Region the request was
//! signed for, over the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so this crate names no other
//! service's crate and nothing leaves the process. Nothing about the placement is minted,
//! derived or defaulted here: what this reports is read off the record Amazon EKS answered
//! with, so an id this service reports is an id the owner of that resource holds.
//!
//! A hop that could NOT be made — no Amazon EKS in this process, a container cluster it
//! holds nothing for, an answer that could not be read — is evidence of nothing, and this
//! service only REPORTS a placement rather than placing anything in it: so it reports
//! none, and the endpoint's `securityGroup` and `subnetIds` stay absent, exactly as they
//! were before anything asked. The create is not refused on silence (that refusal would be
//! assumed rather than grounded) and `services/common/src/network.rs` is the contract this
//! follows.

/// The most subnets ONE endpoint reports as its placement.
///
/// CHOSEN as a process-safety backstop, not a published AWS quota (the modelled `SubnetIds`
/// declares no `length`): the list arrives from a PEER's answer and is then stored on every
/// endpoint of that container cluster, so without a ceiling the size of one record would be
/// decided elsewhere. Sized well above the handful of Availability Zones an Amazon EKS
/// cluster's subnets span.
const MAX_PLACEMENT_SUBNETS: usize = 32;

/// The placement Amazon EKS reports for one container cluster. Empty — no group and no
/// subnet — when it reported nothing, which is what an endpoint then reports.
#[derive(Debug, Clone, Default)]
pub struct ContainerClusterNetwork {
    /// `resourcesVpcConfig.clusterSecurityGroupId`: "The cluster security group that was
    /// created by Amazon EKS for the cluster", as Amazon EKS reports it.
    pub security_group: Option<String>,
    /// `resourcesVpcConfig.subnetIds`, in the order Amazon EKS reports them.
    pub subnet_ids: Vec<String>,
}

/// An AWS SDK client for Amazon EKS over the in-process callback into this same emulator.
///
/// `region` is the request's own signing region, so the question is asked in the Region the
/// caller addressed. The endpoint URL is only the address the callback's transport routes
/// on IN PROCESS: no socket is opened, no name is resolved and nothing leaves this process.
/// The credentials are the callback's, which sign this in-process hop and never appear in a
/// response.
///
/// The Region a request is signed for is CALLER-SUPPLIED text and here it would end up in
/// an endpoint URL's authority, so it is read as a Region LABEL
/// (`service_common::arn::region_label`, the one place this tree states that grammar)
/// instead of being interpolated as given: `None` for a value that would name an authority
/// the caller chose rather than the peer service this emulator asks.
fn eks(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> Option<::aws_sdk_eks::Client> {
    let region = ::service_common::arn::region_label(region)?;
    let config = ::aws_sdk_eks::Config::builder()
        .behavior_version(::aws_sdk_eks::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_eks::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://eks.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The same goes for caching the identity: the credentials the
        // callback hands over are used directly.
        .retry_config(::aws_sdk_eks::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_eks::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_eks::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_eks::config::StalledStreamProtectionConfig::disabled())
        .build();
    Some(::aws_sdk_eks::Client::from_conf(config))
}

/// The network the container cluster `container_cluster_id` runs in, as the service that
/// owns it reports it.
///
/// ONE hop, about ONE cluster, bounded by [`MAX_PLACEMENT_SUBNETS`] whatever the answer
/// carries. A cluster Amazon EKS reports nothing for — including this emulator not being
/// able to ask it at all — yields an empty placement, and the endpoint reports none: see
/// this module's own documentation for why that is not a refusal.
pub async fn container_cluster_network(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    container_cluster_id: &str,
) -> ContainerClusterNetwork {
    let Some(client) = eks(ctx, region) else {
        ::tracing::warn!("the request names no Region label, so Amazon EKS cannot be asked");
        return ContainerClusterNetwork::default();
    };
    // `ContainerProvider$id` is the container cluster's own identifier, which is the name
    // `eks:DescribeCluster` takes.
    let answer = client.describe_cluster().name(container_cluster_id).send().await;
    let cluster = match answer {
        Ok(output) => output.cluster,
        Err(error) => {
            // At `warn`: an endpoint of a container cluster whose owner cannot be asked
            // reports no placement, and this is WHY — worth seeing in a deployment that
            // mounts this service without Amazon EKS.
            ::tracing::warn!(
                %region,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the container cluster's network could not be resolved against Amazon EKS"
            );
            return ContainerClusterNetwork::default();
        }
    };
    let Some(vpc_config) = cluster.and_then(|cluster| cluster.resources_vpc_config) else {
        return ContainerClusterNetwork::default();
    };
    ContainerClusterNetwork {
        security_group: vpc_config.cluster_security_group_id,
        subnet_ids: vpc_config
            .subnet_ids
            .unwrap_or_default()
            .into_iter()
            .take(MAX_PLACEMENT_SUBNETS)
            .collect(),
    }
}
