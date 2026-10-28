// SPDX-License-Identifier: CC0-1.0

//! Resolving the EC2 resources a request names against what the account has.
//!
//! An EC2 instance, a subnet and a security group are EC2's resources, not this service's:
//! `InstanceId` is a plain string in this service's model and the model declares no set of
//! instance ids (`smithy_query(query="operation",operation="AssociateUser")`), and
//! `Settings$Subnets` / `Settings$SecurityGroupId` carry nothing but a pattern, so the only
//! way a call here can establish that the account has the resource a request names is to
//! ask the service that OWNS it. That is the AWS SDK driven over the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), which routes inside this
//! emulator, so this crate names no other service's crate and nothing leaves the process —
//! the same seam every other service reaches another one through. An instance a client
//! launched with `ec2:RunInstances` is therefore a target this service's user-association
//! plane accepts, and a subnet it created with `ec2:CreateSubnet` one a registration can
//! provision its VPC endpoints in, exactly as in the real service.
//!
//! Two questions are asked here, and nothing is cached for either: a resource created a
//! moment ago resolves at once, and one that has been deleted stops resolving at once.
//!
//! * the INSTANCE a user association acts on ([`authorizes_instance`]). The fleet this
//!   service holds of its own
//!   ([`crate::state::ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState::account_instances`])
//!   is consulted FIRST: it costs no hop, and it is what answers in a process that mounts
//!   this service with no EC2 beside it. Neither source is authoritative over the other —
//!   an instance either of them reports is one the account has — so the resolution fails
//!   closed only when NEITHER holds the id.
//! * the NETWORK a registration's `Settings` name ([`resolve_settings_network`]), which
//!   this service holds nothing of its own about, so EC2 is the only source.

/// The EC2 client this service resolves a caller-named resource through.
///
/// `region` is the request's own signing region, so the question is asked in the Region the
/// caller addressed. The client is given the callback's own HTTP client, so the endpoint URL
/// below is only the address that transport routes on IN PROCESS: no socket is opened, no
/// name is resolved and nothing leaves this process. The credentials are the callback's,
/// which sign this in-process hop and never appear in a response.
fn ec2(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_ec2::Client {
    let config = ::aws_sdk_ec2::Config::builder()
        .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_ec2::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://ec2.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The same goes for caching the identity: the credentials the
        // callback hands over are used directly.
        .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_ec2::Client::from_conf(config)
}

/// Whether the account has the EC2 instance `instance_id`, as the service that owns
/// instances reports it.
///
/// ONE instance is asked about, by id, so the work is independent of how many instances the
/// account has. `false` whenever EC2 cannot be asked or answers that it holds no such
/// instance: nothing about an instance is invented here, and an instance that cannot be
/// shown to be the caller's is not one this service acts on.
///
/// An instance EC2 reports in the terminal `terminated` state is NOT one the account has:
/// it is reported for a while after it is gone, and it runs nothing a user-based
/// subscription could be served from. Every other state is an instance the account has.
async fn ec2_holds_instance(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    instance_id: &str,
) -> bool {
    let answer = ec2(ctx, region)
        .describe_instances()
        .instance_ids(instance_id)
        .send()
        .await;
    match answer {
        Ok(output) => output
            .reservations()
            .iter()
            .flat_map(::aws_sdk_ec2::types::Reservation::instances)
            .any(|instance| {
                instance.instance_id() == Some(instance_id)
                    && instance.state().and_then(::aws_sdk_ec2::types::InstanceState::name)
                        != Some(&::aws_sdk_ec2::types::InstanceStateName::Terminated)
            }),
        Err(error) => {
            // At `debug`: an instance EC2 does not hold is the ordinary answer to this
            // question (it is how an unheld id is refused), so a failed hop is not a
            // service fault — but it IS why a request was refused, so it is traceable.
            ::tracing::debug!(
                %region,
                %instance_id,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the EC2 instance a user association names could not be resolved"
            );
            false
        }
    }
}

/// Whether the caller may be shown to have the EC2 instance `instance_id`: true exactly
/// when the account has it, in either of the two places an instance of this account can be
/// reported from — this service's own fleet seam, or EC2 itself.
///
/// The gate every operation that acts on a caller-named instance runs BEFORE it looks
/// anything else up, so a refusal discloses nothing about this service's own resources.
/// Asked in one place, so no two operations can resolve the same instance differently.
pub async fn authorizes_instance<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    instance_id: &str,
) -> bool {
    // The lock is taken and released here, before the hop below: the store is never held
    // across an await.
    let held_locally = crate::state::service_state(engine)
        .is_some_and(|state| state.lock().authorizes_instance(instance_id));
    if held_locally {
        return true;
    }
    let region = crate::state::request_region(&ctx.parts().headers);
    ec2_holds_instance(ctx, &region, instance_id).await
}

/// The `DescribeSubnets` filter that selects subnets BY ID, and the `DescribeSecurityGroups`
/// one that selects groups by id (`subnet-id`, `group-id`, each listed by its operation's
/// own `Filters` documentation).
///
/// A filter is what asks the question, rather than `SubnetIds`/`GroupIds`: an id the account
/// has no resource for is then an absent row instead of a failure of the whole call, so the
/// resolution below can name the id that could not be resolved.
const SUBNET_ID_FILTER: &str = "subnet-id";
const GROUP_ID_FILTER: &str = "group-id";

/// The most subnet ids ONE hop asks about, as values of one `subnet-id` filter.
///
/// The owner evaluates a filter against every subnet the account stores, so the cost of a
/// hop is (values asked x subnets stored): this is the published cap on the first factor —
/// the per-filter value count EC2's own describes refuse above — so one hop costs a
/// constant times what the account stores rather than something the request chooses.
const SUBNET_IDS_PER_HOP: usize = 64;

/// The most DISTINCT subnet ids one request may have resolved, over all of its hops.
///
/// The cap on the second factor, and the one that makes the whole resolution constant-time
/// in the request: it is the most resource ids a describe of the owner accepts in one
/// request at all, so a request naming more ids than that names more than the service that
/// owns subnets would answer about. A request past it is refused rather than served, which
/// is what keeps the ids-named x subnets-stored product bounded by constants whatever the
/// request body carries.
const MAX_SUBNET_IDS_RESOLVED: usize = 1_000;

/// The VPC of each subnet of `subnet_ids` the account has, by subnet id — a subnet it does
/// not have is simply absent, and `None` for the VPC is a subnet EC2 reports without one.
///
/// `None` when EC2 could not be asked at all: that is EVIDENCE OF NOTHING, and the caller
/// refuses nothing on it.
///
/// ONE hop, for AT MOST [`SUBNET_IDS_PER_HOP`] ids: the owner matches the filter against
/// every subnet the account stores, so the work of one hop is that constant times what is
/// stored, and never the length of a request. [`resolved_subnet_vpcs`] is what splits a
/// longer list into hops and what caps how many of them one request may spend.
async fn account_subnets(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[&str],
) -> Option<::std::collections::HashMap<String, Option<String>>> {
    let by_id = ::aws_sdk_ec2::types::Filter::builder()
        .name(SUBNET_ID_FILTER)
        .set_values(Some(
            subnet_ids.iter().map(|id| (*id).to_string()).collect(),
        ))
        .build();
    match ec2(ctx, region)
        .describe_subnets()
        .filters(by_id)
        .send()
        .await
    {
        Ok(output) => Some(
            output
                .subnets()
                .iter()
                .filter_map(|subnet| {
                    subnet
                        .subnet_id()
                        .map(|id| (id.to_string(), subnet.vpc_id().map(str::to_string)))
                })
                .collect(),
        ),
        Err(error) => {
            // At `debug`, for the reason the instance hop logs there: a subnet the account
            // does not have is the ordinary answer to this question, so a failed hop is not
            // a service fault — but it IS why a request was not resolved, so it is
            // traceable.
            ::tracing::debug!(
                %region,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the subnets a registration's settings name could not be resolved"
            );
            None
        }
    }
}

/// What the owner of security groups answered about ONE group id.
enum GroupResolution {
    /// EC2 could not be asked — evidence of nothing.
    NotAsked,
    /// The account has no security group of that id.
    Absent,
    /// The account has it, in the VPC named when EC2 reports one.
    Held(Option<String>),
}

/// What the account's own Amazon EC2 reports for the security group `group_id` names.
async fn account_security_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    group_id: &str,
) -> GroupResolution {
    let by_id = ::aws_sdk_ec2::types::Filter::builder()
        .name(GROUP_ID_FILTER)
        .values(group_id)
        .build();
    match ec2(ctx, region)
        .describe_security_groups()
        .filters(by_id)
        .send()
        .await
    {
        Ok(output) => output
            .security_groups()
            .iter()
            .find(|group| group.group_id() == Some(group_id))
            .map_or(GroupResolution::Absent, |group| {
                GroupResolution::Held(group.vpc_id().map(str::to_string))
            }),
        Err(error) => {
            ::tracing::debug!(
                %region,
                %group_id,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the security group a registration's settings name could not be resolved"
            );
            GroupResolution::NotAsked
        }
    }
}

/// How a refusal names a subnet the account has no subnet of that id for.
///
/// Built from the id the REQUEST sent, in the sentence the service that OWNS subnets states
/// for an id it does not hold, so the caller reads the same fact however it reached the
/// lookup.
fn subnet_does_not_exist(subnet_id: &str) -> String {
    format!("The subnet ID '{subnet_id}' does not exist")
}

/// How a refusal names a security group the account has none of, in that service's own
/// sentence for the same fact. See [`subnet_does_not_exist`].
fn security_group_does_not_exist(group_id: &str) -> String {
    format!("The security group '{group_id}' does not exist")
}

/// What the owner of subnets answered about the ids one member named.
enum SubnetResolution {
    /// EC2 could not be asked — evidence of nothing.
    NotAsked,
    /// The account has every id named; the VPC of each, in the order named. A `None` is a
    /// subnet EC2 reports without a VPC of its own.
    Resolved(Vec<Option<String>>),
}

/// The VPC of each subnet `subnet_ids` names, in the order named and with duplicates
/// collapsed (a subnet named twice is one subnet).
///
/// `Err` naming the first id the ACCOUNT has no subnet for — the existence rule every
/// member that names a subnet is held to — and `Err` for a `member` naming more distinct
/// subnets than [`MAX_SUBNET_IDS_RESOLVED`].
///
/// BOUNDED BY CONSTANTS, not by the request: at most [`MAX_SUBNET_IDS_RESOLVED`] distinct
/// ids are resolved at all, in hops of at most [`SUBNET_IDS_PER_HOP`] ids each, so both
/// factors of the owner's matching cost are capped before the first hop is made. `member`
/// names the request member the ids came from, so a refusal points at what the caller
/// wrote.
async fn resolved_subnet_vpcs(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    member: &str,
    subnet_ids: &[String],
) -> Result<SubnetResolution, crate::state::StateError> {
    let mut named: Vec<&str> = Vec::new();
    let mut seen: ::std::collections::HashSet<&str> = ::std::collections::HashSet::new();
    for subnet_id in subnet_ids {
        if seen.insert(subnet_id.as_str()) {
            named.push(subnet_id.as_str());
        }
    }
    if named.len() > MAX_SUBNET_IDS_RESOLVED {
        return Err(crate::state::StateError::Validation(format!(
            "{member} names {} subnets; at most {MAX_SUBNET_IDS_RESOLVED} may be named in \
             one request",
            named.len()
        )));
    }
    if named.is_empty() {
        return Ok(SubnetResolution::Resolved(Vec::new()));
    }
    let mut held: ::std::collections::HashMap<String, Option<String>> =
        ::std::collections::HashMap::new();
    for chunk in named.chunks(SUBNET_IDS_PER_HOP) {
        let Some(answered) = account_subnets(ctx, region, chunk).await else {
            return Ok(SubnetResolution::NotAsked);
        };
        held.extend(answered);
    }
    let mut vpcs: Vec<Option<String>> = Vec::with_capacity(named.len());
    for subnet_id in &named {
        let Some(holder) = held.get(*subnet_id) else {
            return Err(crate::state::StateError::Validation(subnet_does_not_exist(
                subnet_id,
            )));
        };
        vpcs.push(holder.clone());
    }
    Ok(SubnetResolution::Resolved(vpcs))
}

/// The ONE VPC a set of subnets agrees on, `Err` when two of them are in different VPCs.
///
/// A subnet EC2 reports without a VPC of its own cannot disagree with anything, so it
/// decides nothing here; a set EC2 reports no VPC for at all agrees on `None`.
fn agreed_vpc(vpcs: &[Option<String>]) -> Result<Option<String>, crate::state::StateError> {
    let mut agreed: Option<&str> = None;
    for holder in vpcs.iter().filter_map(Option::as_deref) {
        match agreed {
            None => agreed = Some(holder),
            Some(expected) if expected == holder => {}
            Some(expected) => {
                return Err(crate::state::StateError::Validation(format!(
                    "All subnets must belong to the same VPC: '{expected}'"
                )));
            }
        }
    }
    Ok(agreed.map(str::to_string))
}

/// Resolve the network resources a registration's `Settings` name against what the account
/// has.
///
/// `Subnets` is "the ID of one or more subnets in which License Manager will create a VPC
/// endpoint for products that require connectivity to activation servers" and
/// `SecurityGroupId` "a security group ID that allows inbound TCP port 1688 communication
/// between resources in your VPC and the VPC endpoint for activation servers": the
/// registration PROVISIONS those VPC endpoints, so every id it carries names a network
/// resource the account already has, and a VPC endpoint exists in ONE VPC ("your VPC"), so
/// the subnets and the security group of one registration are resources of one VPC.
///
/// An EMPTY `SecurityGroupId` is the configuration a registration that supplied no
/// `Settings` at all carries — the member's `@pattern` admits no empty value, so no request
/// can spell one — and names no group to resolve; neither does an empty subnet list.
///
/// BOUNDED: at most two kinds of hop, the subnets' and the group's, each capped by
/// constants rather than by the request — see [`resolved_subnet_vpcs`] — and no lock of
/// this service held across any of them. The two are INDEPENDENT: a group the account does
/// not have is refused whether or not the subnets could be asked about, and only the VPC
/// the two have to agree on goes unchecked when one of them could not be.
pub async fn resolve_settings_network(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    settings: &crate::ops::types::Settings,
) -> Result<(), crate::state::StateError> {
    let region = crate::state::request_region(&ctx.parts().headers);
    // The VPC this registration's endpoints are created in: the one its subnets agree on,
    // and nothing when the subnets could not be asked about at all — then there is simply
    // no VPC for the group below to be compared against.
    let endpoint_vpc =
        match resolved_subnet_vpcs(ctx, &region, SETTINGS_SUBNETS_MEMBER, &settings.subnets).await?
        {
            SubnetResolution::NotAsked => None,
            SubnetResolution::Resolved(vpcs) => agreed_vpc(&vpcs)?,
        };

    if settings.security_group_id.is_empty() {
        return Ok(());
    }
    let group_vpc = match account_security_group(ctx, &region, &settings.security_group_id).await {
        GroupResolution::NotAsked => return Ok(()),
        GroupResolution::Absent => {
            return Err(crate::state::StateError::Validation(
                security_group_does_not_exist(&settings.security_group_id),
            ));
        }
        GroupResolution::Held(vpc_id) => vpc_id,
    };
    // A security group is scoped to one VPC (`SecurityGroup$VpcId`), so a group of another
    // VPC cannot carry the traffic of the endpoint these subnets put it in front of.
    if let (Some(expected), Some(holder)) = (endpoint_vpc.as_deref(), group_vpc.as_deref()) {
        if expected != holder {
            return Err(crate::state::StateError::Validation(format!(
                "The security group '{}' must belong to the same VPC as the subnets: \
                 '{expected}'",
                settings.security_group_id
            )));
        }
    }
    Ok(())
}

/// Resolve the subnets a SELF-MANAGED Active Directory's network settings name against what
/// the account has.
///
/// `DomainNetworkSettings.Subnets` is "a list of subnets that apply for the Active Directory
/// domain" (`@required`, `@length(min: 1)`), so each one names a subnet of the account the
/// same way `Settings$Subnets` does, and the same existence rule holds for it.
///
/// Existence ONLY: nothing is provisioned in these subnets — they are where the domain is
/// reached — so there is no VPC endpoint to confine to one VPC, and a domain reachable in
/// more than one VPC is not something this member rules out.
pub async fn resolve_domain_subnets(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    identity_provider: &crate::ops::types::IdentityProvider,
) -> Result<(), crate::state::StateError> {
    let crate::ops::types::IdentityProvider::ActiveDirectoryIdentityProvider(ad) =
        identity_provider;
    let Some(subnets) = ad
        .active_directory_settings
        .as_ref()
        .and_then(|settings| settings.domain_network_settings.as_ref())
        .map(|network| &network.subnets)
    else {
        return Ok(());
    };
    let region = crate::state::request_region(&ctx.parts().headers);
    resolved_subnet_vpcs(ctx, &region, DOMAIN_SUBNETS_MEMBER, subnets).await?;
    Ok(())
}

/// The request members a resolved subnet id comes from, as a refusal names them.
const SETTINGS_SUBNETS_MEMBER: &str = "Settings.Subnets";
const DOMAIN_SUBNETS_MEMBER: &str =
    "ActiveDirectorySettings.DomainNetworkSettings.Subnets";
