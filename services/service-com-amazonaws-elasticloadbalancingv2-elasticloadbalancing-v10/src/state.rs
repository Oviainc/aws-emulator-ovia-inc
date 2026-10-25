// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10 service.
//!
//! Everything the ELBv2 provisioning loop persists lives in the ONE guarded
//! [`ComAmazonawsElasticloadbalancingv2ElasticloadbalancingV10State`] value, so every
//! handler in this crate sees the same store across requests: load balancers, their
//! listeners, target groups and the targets registered with them, and the trust
//! stores a mutual-TLS listener authenticates against.
//!
//! Records are the service's own storage shape; each one converts to its modelled
//! Smithy shape through a `to_shape` method, so a describe answers exactly what the
//! create persisted instead of re-deriving it.
//!
//! One set of facts a request is resolved against is NOT this service's: the account's
//! VPCs, subnets, security groups and instances. [`network`] resolves those against EC2,
//! which owns them.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

pub mod network;

/// The unit tests of [`network`]'s pure half — where an address falls relative to a VPC.
#[cfg(test)]
mod vpc_topology;

use crate::ops::types::{
    Action, AvailabilityZone, Certificate, IpamPools, Listener, ListenerAttribute, LoadBalancer,
    LoadBalancerAddress, LoadBalancerAttribute, LoadBalancerState, Matcher, Rule, RuleCondition,
    RuleTransform, SubnetMapping, TargetDescription, TargetGroup, TargetGroupAttribute,
    TargetHealth, TargetHealthDescription, TrustStore,
};

/// How long after the call that starts it a load balancer owes its move out of
/// `provisioning`, and a freshly registered target its move out of `initial`.
///
/// A chosen span (AWS takes minutes): the emulator only has to make the transient
/// state observable to a caller acting straight after the call, so it is inside the
/// 250–750 ms band the project fixes for a clock-owed transition. The due instant is
/// stored on the record; nothing ever sleeps for it.
pub const SETTLE_AFTER: Duration = Duration::from_millis(500);

/// The default region for a request that carries no SigV4 credential scope to read
/// one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How many load balancers this store holds before `CreateLoadBalancer` refuses with
/// the modelled `TooManyLoadBalancersException`.
///
/// AWS publishes an adjustable per-account, per-Region quota, and this emulator models
/// neither Service Quotas nor an account's adjustments — so this is the emulator's OWN
/// bound on the store rather than a claim about any account's limit. It is far above any
/// workflow an emulator serves, and it is what stops the store growing without end:
/// every record it holds is released by `DeleteLoadBalancer`.
pub const LOAD_BALANCER_QUOTA: usize = 1_000;

/// How many target groups this store holds before `CreateTargetGroup` refuses with the
/// modelled `TooManyTargetGroupsException`. The emulator's own bound, as
/// [`LOAD_BALANCER_QUOTA`].
pub const TARGET_GROUP_QUOTA: usize = 1_000;

/// How many listeners one load balancer carries before `CreateListener` refuses with
/// the modelled `TooManyListenersException`. The emulator's own bound, as
/// [`LOAD_BALANCER_QUOTA`]; a listener exists only under a load balancer, so the
/// listener store is bounded by this times [`LOAD_BALANCER_QUOTA`], and deleting a load
/// balancer releases its listeners.
pub const LISTENERS_PER_LOAD_BALANCER: usize = 100;

/// How many rules one load balancer carries across its listeners before `CreateRule`
/// refuses with the modelled `TooManyRulesException`: the published default
/// `DescribeAccountLimits` reports as `rules-per-application-load-balancer` (and as
/// `rules-per-network-load-balancer`), which counts the rules a caller added and not the
/// default rule every listener carries.
///
/// A rule exists only under a listener, so the store's rules are bounded by this times
/// [`LOAD_BALANCER_QUOTA`], and deleting a listener or its load balancer releases them.
pub const RULES_PER_LOAD_BALANCER: usize = 100;

/// How many values one condition of a rule carries, from the published default
/// `DescribeAccountLimits` reports as `condition-values-per-alb-rule`.
pub const CONDITION_VALUES_PER_RULE: usize = 5;

/// How many targets one target group of an APPLICATION Load Balancer holds before
/// `RegisterTargets` refuses with the modelled `TooManyTargetsException`: the per-Region
/// default Elastic Load Balancing publishes for "targets per target group", which
/// `DescribeAccountLimits` reports under that name. Registrations live on their target
/// group, so the store's targets are bounded by this times [`TARGET_GROUP_QUOTA`].
pub const TARGETS_PER_TARGET_GROUP: usize = 1_000;

/// How many targets one target group of a NETWORK Load Balancer holds, from the published
/// default `DescribeAccountLimits` reports as
/// `targets-per-availability-zone-per-network-load-balancer`.
///
/// AWS publishes that number PER Availability Zone, and this emulator models no per-zone
/// accounting, so it is applied to the group as a whole: an approximation that is never
/// more permissive than the published quota, and the number a caller reads for that name.
pub const TARGETS_PER_NETWORK_TARGET_GROUP: usize = 500;

/// How many targets one target group of a GATEWAY Load Balancer holds, from the published
/// default `DescribeAccountLimits` reports as
/// `targets-per-availability-zone-per-gateway-load-balancer`, applied to the group as a
/// whole for the reason [`TARGETS_PER_NETWORK_TARGET_GROUP`] gives.
pub const TARGETS_PER_GATEWAY_TARGET_GROUP: usize = 300;

/// How many targets a target group whose targets are not addressed by the load balancer
/// at all holds: ONE.
///
/// A `lambda` target group carries a single function (`TargetGroup$TargetType`: "lambda -
/// Register a single Lambda function as a target") and an `alb` one a single Application
/// Load Balancer ("alb - Register a single Application Load Balancer as a target").
pub const TARGETS_PER_SINGLETON_TARGET_GROUP: usize = 1;

/// How many targets `group` holds before `RegisterTargets` refuses the registration that
/// would exceed it, which is the quota of the load balancer family the group serves — see
/// [`TARGETS_PER_TARGET_GROUP`], [`TARGETS_PER_NETWORK_TARGET_GROUP`],
/// [`TARGETS_PER_GATEWAY_TARGET_GROUP`] and [`TARGETS_PER_SINGLETON_TARGET_GROUP`].
pub fn targets_per_target_group(group: &TargetGroupRecord) -> usize {
    if matches!(group.target_type.as_str(), "lambda" | "alb") {
        return TARGETS_PER_SINGLETON_TARGET_GROUP;
    }
    match group.family() {
        LoadBalancerFamily::Application => TARGETS_PER_TARGET_GROUP,
        LoadBalancerFamily::Network => TARGETS_PER_NETWORK_TARGET_GROUP,
        LoadBalancerFamily::Gateway => TARGETS_PER_GATEWAY_TARGET_GROUP,
    }
}

/// The ARN/endpoint namespace for this service (`sigv4Name` / `arnNamespace` in the
/// model's `aws.api#service` trait).
pub const SERVICE_NAMESPACE: &str = "elasticloadbalancing";

/// The region this request was signed for, which is the region every ARN and DNS
/// name minted while serving it belongs to.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// One availability zone a load balancer is attached to, as the subnet that put it
/// there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AvailabilityZoneRecord {
    pub zone_name: String,
    pub subnet_id: String,
    pub outpost_id: Option<String>,
    pub addresses: Vec<LoadBalancerAddressRecord>,
}

impl AvailabilityZoneRecord {
    /// This zone as the modelled shape, which `DescribeLoadBalancers` reports and
    /// `SetSubnets` answers with.
    pub fn to_shape(&self) -> AvailabilityZone {
        AvailabilityZone {
            zone_name: Some(self.zone_name.clone()),
            subnet_id: Some(self.subnet_id.clone()),
            outpost_id: self.outpost_id.clone(),
            load_balancer_addresses: Some(self.addresses.iter().map(|a| a.to_shape()).collect()),
            source_nat_ipv6_prefixes: None,
        }
    }
}

/// An address a caller pinned to one of the load balancer's zones through
/// `SubnetMappings`. Only what the caller supplied is kept: the emulator does not
/// model an address pool, so it never invents an `IpAddress`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadBalancerAddressRecord {
    pub allocation_id: Option<String>,
    pub private_ipv4_address: Option<String>,
    pub ipv6_address: Option<String>,
}

impl LoadBalancerAddressRecord {
    /// Whether the caller pinned nothing on this address, so there is nothing to
    /// report for the zone.
    pub fn is_empty(&self) -> bool {
        self.allocation_id.is_none()
            && self.private_ipv4_address.is_none()
            && self.ipv6_address.is_none()
    }

    fn to_shape(&self) -> LoadBalancerAddress {
        LoadBalancerAddress {
            ip_address: None,
            allocation_id: self.allocation_id.clone(),
            private_i_pv4_address: self.private_ipv4_address.clone(),
            i_pv6_address: self.ipv6_address.clone(),
        }
    }
}

/// A persisted load balancer.
#[derive(Debug, Clone)]
pub struct LoadBalancerRecord {
    pub arn: String,
    /// The 16-hex id that both the ARN and the DNS name carry.
    pub id: String,
    pub name: String,
    pub dns_name: String,
    pub created_time: SystemTime,
    pub scheme: String,
    pub vpc_id: Option<String>,
    pub state_code: String,
    /// When the clock owes this load balancer its move to `active`.
    pub active_at: SystemTime,
    pub lb_type: String,
    pub availability_zones: Vec<AvailabilityZoneRecord>,
    pub security_groups: Vec<String>,
    pub ip_address_type: String,
    pub customer_owned_ipv4_pool: Option<String>,
    /// Whether inbound security group rules are evaluated for PrivateLink traffic, as a
    /// caller last set it through `SetSecurityGroups`. `None` until one does, which is
    /// when the documented default applies
    /// ([`Self::enforce_security_group_inbound_rules_on_private_link_traffic`]).
    pub enforce_security_group_inbound_rules_on_private_link_traffic: Option<String>,
    /// Whether an IPv6 prefix of each subnet is used for source NAT, as `SetSubnets`
    /// last set it. `None` until it does: the member's documented default is `off`.
    pub enable_prefix_for_ipv6_source_nat: Option<String>,
    /// The IPAM pool the load balancer draws its IPv4 addresses from
    /// (`IpamPools$Ipv4IpamPoolId`), as `CreateLoadBalancer` or `ModifyIpPools` last set
    /// it; `None` for a load balancer using no pool.
    pub ipv4_ipam_pool_id: Option<String>,
    /// When a capacity-reservation write last changed this load balancer's reservation,
    /// which `DescribeCapacityReservation` reports as `LastModifiedTime`. `None` until
    /// one does: a load balancer nobody has written a reservation for has no such
    /// instant to report.
    pub capacity_reservation_modified_at: Option<SystemTime>,
    pub attributes: HashMap<String, String>,
    pub tags: HashMap<String, String>,
    pub listener_arns: Vec<String>,
}

impl LoadBalancerRecord {
    /// The settings `CreateLoadBalancer` is idempotent over: the model documents
    /// that creating the same load balancer twice with the same settings succeeds,
    /// so a repeat call compares these and returns the stored resource.
    pub fn has_settings_of(&self, other: &LoadBalancerRecord) -> bool {
        self.scheme == other.scheme
            && self.lb_type == other.lb_type
            && self.ip_address_type == other.ip_address_type
            && self.customer_owned_ipv4_pool == other.customer_owned_ipv4_pool
            && self.ipv4_ipam_pool_id == other.ipv4_ipam_pool_id
            && self.availability_zones == other.availability_zones
            && sorted(&self.security_groups) == sorted(&other.security_groups)
            && self.tags == other.tags
    }

    pub fn to_shape(&self) -> LoadBalancer {
        LoadBalancer {
            load_balancer_arn: Some(self.arn.clone()),
            dns_name: Some(self.dns_name.clone()),
            // AWS reports the PUBLISHED Route 53 hosted zone id of the load
            // balancer's region and type. That constant is in neither the model nor
            // the request, and a derived stand-in would be a value of the right
            // shape that resolves to nothing — so, as with `vpc_id`, the member is
            // left unreported rather than invented.
            canonical_hosted_zone_id: None,
            created_time: Some(::aws_smithy_types::DateTime::from(self.created_time)),
            load_balancer_name: Some(self.name.clone()),
            scheme: Some(self.scheme.clone()),
            vpc_id: self.vpc_id.clone(),
            state: Some(LoadBalancerState {
                code: Some(self.state_code.clone()),
                reason: None,
            }),
            r#type: Some(self.lb_type.clone()),
            availability_zones: Some(
                self.availability_zones
                    .iter()
                    .map(AvailabilityZoneRecord::to_shape)
                    .collect(),
            ),
            security_groups: Some(self.security_groups.clone()),
            ip_address_type: Some(self.ip_address_type.clone()),
            customer_owned_ipv4_pool: self.customer_owned_ipv4_pool.clone(),
            enforce_security_group_inbound_rules_on_private_link_traffic: self
                .enforce_security_group_inbound_rules_on_private_link_traffic(),
            enable_prefix_for_ipv6_source_nat: self.enable_prefix_for_ipv6_source_nat.clone(),
            ipam_pools: self.ipam_pools_shape(),
        }
    }

    /// Which family of load balancer this is, which decides the attributes it carries
    /// and the IP address types and security groups it accepts.
    pub fn family(&self) -> LoadBalancerFamily {
        load_balancer_family(&self.lb_type)
    }

    /// The IPAM pools this load balancer draws from, as the modelled shape, or `None`
    /// for one that draws from no pool — the member is then left unreported rather than
    /// reported as an empty structure.
    pub fn ipam_pools_shape(&self) -> Option<IpamPools> {
        self.ipv4_ipam_pool_id
            .clone()
            .map(|pool| IpamPools {
                ipv4_ipam_pool_id: Some(pool),
            })
    }

    /// Whether inbound security group rules are evaluated for PrivateLink traffic: what
    /// a caller set, else the documented default for a load balancer the member applies
    /// to at all.
    ///
    /// `SetSecurityGroupsInput$EnforceSecurityGroupInboundRulesOnPrivateLinkTraffic`:
    /// "Indicates whether to evaluate inbound security group rules for traffic sent to a
    /// Network Load Balancer through Amazon Web Services PrivateLink. Applies only if the
    /// load balancer has an associated security group. The default is on." So it is
    /// reported for a Network Load Balancer carrying a security group, and for no other.
    pub fn enforce_security_group_inbound_rules_on_private_link_traffic(&self) -> Option<String> {
        if let Some(set) = self
            .enforce_security_group_inbound_rules_on_private_link_traffic
            .clone()
        {
            return Some(set);
        }
        (self.family() == LoadBalancerFamily::Network && !self.security_groups.is_empty())
            .then(|| PRIVATE_LINK_INBOUND_RULES_DEFAULT.to_string())
    }

    /// Every attribute this load balancer reports: each one the model documents for a
    /// load balancer of this family, carrying what a caller set or, failing that, its
    /// documented default. Sorted by key, so two reads of one load balancer answer
    /// alike.
    pub fn effective_attributes(&self) -> Vec<(String, String)> {
        let mut reported: Vec<(String, String)> = LOAD_BALANCER_ATTRIBUTES
            .iter()
            .filter(|spec| spec.applies_to(self))
            .filter_map(|spec| {
                let value = self
                    .attributes
                    .get(spec.key)
                    .cloned()
                    .or_else(|| spec.default_for(self))?;
                Some((spec.key.to_string(), value))
            })
            .collect();
        reported.sort_by(|left, right| left.0.cmp(&right.0));
        reported
    }

    /// [`Self::effective_attributes`] as the modelled shape.
    pub fn attribute_shapes(&self) -> Vec<LoadBalancerAttribute> {
        self.effective_attributes()
            .into_iter()
            .map(|(key, value)| LoadBalancerAttribute {
                key: Some(key),
                value: Some(value),
            })
            .collect()
    }
}

/// The health-check settings of a target group, with every default materialized at
/// create time so a later read reports the same values AWS would.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HealthCheckConfig {
    pub enabled: bool,
    pub protocol: Option<String>,
    pub port: Option<String>,
    pub path: Option<String>,
    pub interval_seconds: i32,
    pub timeout_seconds: i32,
    pub healthy_threshold: i32,
    pub unhealthy_threshold: i32,
    pub matcher: Option<MatcherRecord>,
}

/// The success codes of an HTTP/HTTPS health check.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MatcherRecord {
    pub http_code: Option<String>,
    pub grpc_code: Option<String>,
}

impl MatcherRecord {
    /// Whether this matcher names a return code at all — an HTTP one or a gRPC one.
    ///
    /// The model gives `Matcher` two optional members (`HttpCode`, `GrpcCode`), so a
    /// matcher carrying neither states no success criterion and is no matcher as far as
    /// a health check is concerned.
    pub fn names_a_return_code(&self) -> bool {
        self.http_code.is_some() || self.grpc_code.is_some()
    }

    fn to_shape(&self) -> Matcher {
        Matcher {
            http_code: self.http_code.clone(),
            grpc_code: self.grpc_code.clone(),
        }
    }
}

/// Whether a layer-7 (HTTP-semantic) health check made of `path` and `matcher` is
/// complete — it has a destination to request and a return code that counts as success —
/// and, when it is not, the rule it broke.
///
/// Both members are documented as belonging to such a check alone (`HealthCheckPath`:
/// "[HTTP/HTTPS health checks] The destination for health checks on the targets";
/// `Matcher`: "[HTTP/HTTPS health checks] The HTTP or gRPC codes to use when checking
/// for a successful response from a target"), and a check that requests nothing, or
/// accepts no response as success, cannot be evaluated at all. Only a path for the load
/// balancer to request and a code to compare its answer against make one, so the two are
/// required TOGETHER of any configuration that is not filling their documented defaults.
///
/// The message states the rule rather than the request, because neither member is
/// identified by a value the caller sent: what is missing is a member, not a bad value.
pub fn check_layer_7_health_check(
    path: Option<&str>,
    matcher: Option<&MatcherRecord>,
) -> Result<(), String> {
    if path.is_some() && matcher.is_some_and(MatcherRecord::names_a_return_code) {
        return Ok(());
    }
    Err("Path and return code are required for layer 7 health checks.".to_string())
}

/// The path an HTTP-semantic health check checks when the request names none, from
/// `HealthCheckPath`'s own documentation: "[HTTP1 or HTTP2 protocol version] The ping
/// path. The default is /. [GRPC protocol version] The path of a custom health check
/// method with the format /package.service/method. The default is
/// /AWS.ALB/healthcheck."
pub fn default_health_check_path(protocol_version: Option<&str>) -> String {
    if protocol_version == Some("GRPC") {
        "/AWS.ALB/healthcheck".to_string()
    } else {
        "/".to_string()
    }
}

/// The response codes an HTTP-semantic health check accepts when the request names
/// none: the gRPC success code `12` for a gRPC target group, else the default
/// `Matcher$HttpCode` documents for the load balancer family the group serves — "For
/// Application Load Balancers, you can specify values between 200 and 499, with the
/// default value being 200. For Network Load Balancers, you can specify values between
/// 200 and 599, with the default value being 200-399. For Gateway Load Balancers, this
/// must be "200-399"".
///
/// A health check's response codes therefore do NOT follow the protocol the check speaks:
/// a Network Load Balancer's target group whose health check is an HTTP one still accepts
/// that family's whole success range.
pub fn default_health_check_matcher(
    protocol_version: Option<&str>,
    family: LoadBalancerFamily,
) -> MatcherRecord {
    if protocol_version == Some("GRPC") {
        return MatcherRecord {
            http_code: None,
            grpc_code: Some("12".to_string()),
        };
    }
    let http_code = match family {
        LoadBalancerFamily::Application => "200",
        LoadBalancerFamily::Network | LoadBalancerFamily::Gateway => "200-399",
    };
    MatcherRecord {
        http_code: Some(http_code.to_string()),
        grpc_code: None,
    }
}

/// The message Elastic Load Balancing answers a `TargetGroupNotFound` with for an
/// operation naming the target group it acts on — one `TargetGroupArn`: the identifier
/// the request carried, quoted, under a PLURAL noun whatever its number.
///
/// Built from the request, because the identifier it names is the only thing the caller
/// can act on. The recorded answer to a `DescribeTargetGroupAttributes` of a deleted
/// target group is `Target groups
/// 'arn:aws:elasticloadbalancing:…:targetgroup/…/…' not found` — one identifier under
/// the plural noun.
pub fn target_groups_not_found_message<'a>(
    identifiers: impl IntoIterator<Item = &'a str>,
) -> String {
    let named: Vec<&str> = identifiers.into_iter().collect();
    format!("Target groups '{}' not found", named.join(", "))
}

/// The message Elastic Load Balancing answers a `TargetGroupNotFound` with for a read
/// over a LIST of identifiers (`DescribeTargetGroups`, whose `TargetGroupArns` and
/// `Names` are both lists).
///
/// There the refusal is about the request's whole selection rather than one entry of it,
/// and the service names no value — the same shape of message its sibling list reads
/// answer ('One or more listeners not found'), and what the recorded
/// `DescribeTargetGroups` of an absent ARN answers (a replay of that step accepts this
/// sentence and rejects one naming the ARN, which is why the two wordings differ).
pub fn target_group_selection_not_found_message() -> String {
    "One or more target groups not found".to_string()
}

/// The message Elastic Load Balancing answers a `ListenerNotFound` with for an
/// operation that addresses ONE listener through its own required `ListenerArn` —
/// `ModifyListener`, `DeleteListener`, the certificate writes and the attribute read and
/// write.
///
/// Built from the request: the identifier it names is the only thing the caller can act
/// on, and the recorded answers to a `ModifyListener` and a `DescribeListenerAttributes`
/// of an absent listener quote that ARN back under the singular noun.
pub fn listener_not_found_message(arn: &str) -> String {
    format!("Listener '{arn}' not found")
}

/// The message Elastic Load Balancing answers a `ListenerNotFound` with for a read over
/// a LIST of listeners, or over the rules or certificates of one: there the refusal is
/// about the selection the request asked for rather than one listener the call
/// addresses.
///
/// The service names no value there — the same shape of message its sibling target
/// group reads answer ([`target_group_selection_not_found_message`]), and what the
/// recorded `DescribeListeners`, `DescribeListenerCertificates` and `DescribeRules` of
/// an absent listener answer.
pub fn listener_selection_not_found_message() -> String {
    "One or more listeners not found".to_string()
}

/// The message Elastic Load Balancing answers a `RuleNotFound` with for a read over a
/// LIST of rule identifiers (`DescribeRules`, whose `RuleArns` is a list): the selection
/// the request asked for is what could not be resolved, and the service names no value
/// in it, exactly as its sibling list reads do.
pub fn rule_selection_not_found_message() -> String {
    "One or more rules not found".to_string()
}

/// The message Elastic Load Balancing answers a `RuleNotFound` with for an operation
/// that addresses ONE rule through its own required `RuleArn` (`DeleteRule`).
///
/// Built from the request: the identifier it names is the only thing the caller can act
/// on, exactly as the listener-addressing operations quote their `ListenerArn` back
/// ([`listener_not_found_message`]), while a read over a LIST of rule identifiers names
/// no value ([`rule_selection_not_found_message`]).
pub fn rule_not_found_message(arn: &str) -> String {
    format!("Rule '{arn}' not found")
}

/// The published maximum length of a load balancer name, from `CreateLoadBalancer`'s
/// `Name` documentation: "can have a maximum of 32 characters".
pub const LOAD_BALANCER_NAME_MAX_LENGTH: usize = 32;

/// Whether `name` is a load balancer NAME this service can address, as the model's
/// `CreateLoadBalancerInput$Name` documentation states the rule: "can have a maximum of
/// 32 characters, must contain only alphanumeric characters or hyphens, must not begin
/// or end with a hyphen".
///
/// Read by [`check_listener_arn`]: a load balancer name this service could never have
/// minted is not a name any ARN of this service can carry.
pub fn is_load_balancer_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= LOAD_BALANCER_NAME_MAX_LENGTH
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && !name.starts_with('-')
        && !name.ends_with('-')
}

/// The ARN segments [`load_balancer_type_code`] mints, which are the only load balancer
/// type codes an Elastic Load Balancing ARN carries.
pub const LOAD_BALANCER_TYPE_CODES: &[&str] = &["app", "net", "gwy"];

/// Whether `value` is a resource id of the form every load balancer, listener and rule
/// ARN of this service carries: the 16 hex characters
/// `ComAmazonawsElasticloadbalancingv2ElasticloadbalancingV10State::mint_id` produces.
fn is_resource_id(value: &str) -> bool {
    value.len() == 16 && value.chars().all(|c| c.is_ascii_hexdigit())
}

/// Whether `arn` is a well-formed LISTENER ARN — one this API could address at all —
/// and, when it is not, the refusal naming the value, which is the only thing the caller
/// can act on.
///
/// A listener ARN is `listener/<type>/<load-balancer-name>/<load-balancer-id>/<id>` with
/// a type code this service mints ([`LOAD_BALANCER_TYPE_CODES`]), a load balancer name
/// that satisfies the documented name rule ([`is_load_balancer_name`]) and two resource
/// ids of the form every ARN here carries. A value that breaks any of those names no
/// listener this API can have, whatever the store holds, and is refused as an invalid
/// ARN rather than reported as a missing listener — while a well-formed ARN that simply
/// names nothing is the `ListenerNotFound` of the operation that was called. The
/// partition, Region and account are deliberately NOT checked: an ARN of another Region
/// is well formed and simply names no listener this store holds.
pub fn check_listener_arn(arn: &str) -> Result<(), String> {
    let malformed = || Err(format!("'{arn}' is not a valid listener ARN"));
    let Some(parsed) = ::service_common::arn::parse_of(arn, SERVICE_NAMESPACE) else {
        return malformed();
    };
    let Some(resource_id) = parsed.resource.strip_prefix("listener/") else {
        return malformed();
    };
    let segments: Vec<&str> = resource_id.split('/').collect();
    let [type_code, name, load_balancer_id, listener_id] = segments[..] else {
        return malformed();
    };
    if !LOAD_BALANCER_TYPE_CODES.contains(&type_code)
        || !is_load_balancer_name(name)
        || !is_resource_id(load_balancer_id)
        || !is_resource_id(listener_id)
    {
        return malformed();
    }
    Ok(())
}

/// The listener protocols that terminate TLS at the load balancer, and so are the ones
/// the model brackets the certificate and security-policy members for.
///
/// From this model's own documentation of `CreateListenerInput$Certificates` — "[HTTPS
/// and TLS listeners] The default certificate for the listener. You must provide exactly
/// one certificate." — and of `SslPolicy`, bracketed the same way. `ProtocolEnum`'s other
/// values (GENEVE, HTTP, QUIC, TCP, TCP_QUIC, TCP_UDP, UDP) are outside that bracket, as
/// is a listener that names no protocol at all — the model: "You can't specify a protocol
/// for a Gateway Load Balancer".
pub const TLS_TERMINATING_PROTOCOLS: &[&str] = &["HTTPS", "TLS"];

/// Whether a listener on `protocol` terminates TLS, which is what decides whether it
/// carries a default certificate, a certificate list and a security policy at all.
pub fn terminates_tls(protocol: Option<&str>) -> bool {
    protocol.is_some_and(|value| TLS_TERMINATING_PROTOCOLS.contains(&value))
}

/// How many certificates one listener's certificate list holds before
/// `AddListenerCertificates` refuses with the modelled `TooManyCertificatesException`.
/// The emulator's own bound, as [`LOAD_BALANCER_QUOTA`]: the list lives on the listener
/// record, so it is released with the listener.
pub const CERTIFICATES_PER_LISTENER: usize = 25;

/// Every listener attribute this service models, with the load balancer families whose
/// listeners carry it, the values it accepts and its documented default — each one a
/// sentence of the model's own `ListenerAttribute$Key` documentation
/// (`smithy_query(query="doc",shape="ListenerAttribute",member="Key")`), which is the
/// only catalog of them; the Smithy shape itself is an unconstrained key/value pair, so
/// nothing else in the model lists them.
pub static LISTENER_ATTRIBUTES: &[ListenerAttributeSpec] = &[
    // "The following attribute is supported by Network Load Balancers, and Gateway Load
    // Balancers: tcp.idle_timeout.seconds - The tcp idle timeout value, in seconds. The
    // valid range is 60-6000 seconds. The default is 350 seconds."
    ListenerAttributeSpec {
        key: "tcp.idle_timeout.seconds",
        domain: AttributeDomain::Integer { min: 60, max: 6000 },
        families: &[LoadBalancerFamily::Network, LoadBalancerFamily::Gateway],
        default: Some("350"),
    },
    // "The following attribute is only supported by Gateway Load Balancers:
    // send_tcp_reset.on_idle_timeout.enabled – ... The value is true or false. The
    // default is false."
    ListenerAttributeSpec {
        key: "send_tcp_reset.on_idle_timeout.enabled",
        domain: AttributeDomain::Boolean,
        families: &[LoadBalancerFamily::Gateway],
        default: Some("false"),
    },
    // "The following attributes are only supported by Application Load Balancers": the
    // mutual-TLS and TLS request header names, each of which the documentation describes
    // as a header NAME the caller chooses, so the model states no domain and no default
    // for any of them — they are reported once a caller has set one.
    ListenerAttributeSpec {
        key: "routing.http.request.x_amzn_mtls_clientcert_serial_number.header_name",
        domain: AttributeDomain::Text,
        families: &[LoadBalancerFamily::Application],
        default: None,
    },
    ListenerAttributeSpec {
        key: "routing.http.request.x_amzn_mtls_clientcert_issuer.header_name",
        domain: AttributeDomain::Text,
        families: &[LoadBalancerFamily::Application],
        default: None,
    },
    ListenerAttributeSpec {
        key: "routing.http.request.x_amzn_mtls_clientcert_subject.header_name",
        domain: AttributeDomain::Text,
        families: &[LoadBalancerFamily::Application],
        default: None,
    },
    ListenerAttributeSpec {
        key: "routing.http.request.x_amzn_mtls_clientcert_validity.header_name",
        domain: AttributeDomain::Text,
        families: &[LoadBalancerFamily::Application],
        default: None,
    },
    ListenerAttributeSpec {
        key: "routing.http.request.x_amzn_mtls_clientcert_leaf.header_name",
        domain: AttributeDomain::Text,
        families: &[LoadBalancerFamily::Application],
        default: None,
    },
    ListenerAttributeSpec {
        key: "routing.http.request.x_amzn_mtls_clientcert.header_name",
        domain: AttributeDomain::Text,
        families: &[LoadBalancerFamily::Application],
        default: None,
    },
    ListenerAttributeSpec {
        key: "routing.http.request.x_amzn_tls_version.header_name",
        domain: AttributeDomain::Text,
        families: &[LoadBalancerFamily::Application],
        default: None,
    },
    ListenerAttributeSpec {
        key: "routing.http.request.x_amzn_tls_cipher_suite.header_name",
        domain: AttributeDomain::Text,
        families: &[LoadBalancerFamily::Application],
        default: None,
    },
    // "routing.http.response.server.enabled - Enables you to allow or remove the HTTP
    // response server header." Allowed or removed: the two states the sentence names.
    ListenerAttributeSpec {
        key: "routing.http.response.server.enabled",
        domain: AttributeDomain::Boolean,
        families: &[LoadBalancerFamily::Application],
        default: None,
    },
    // The response header VALUES, each documented as the value of one HTTP response
    // header — again a caller's string, with no domain or default in the model.
    ListenerAttributeSpec {
        key: "routing.http.response.strict_transport_security.header_value",
        domain: AttributeDomain::Text,
        families: &[LoadBalancerFamily::Application],
        default: None,
    },
    ListenerAttributeSpec {
        key: "routing.http.response.access_control_allow_origin.header_value",
        domain: AttributeDomain::Text,
        families: &[LoadBalancerFamily::Application],
        default: None,
    },
    ListenerAttributeSpec {
        key: "routing.http.response.access_control_allow_methods.header_value",
        domain: AttributeDomain::Text,
        families: &[LoadBalancerFamily::Application],
        default: None,
    },
    ListenerAttributeSpec {
        key: "routing.http.response.access_control_allow_headers.header_value",
        domain: AttributeDomain::Text,
        families: &[LoadBalancerFamily::Application],
        default: None,
    },
    ListenerAttributeSpec {
        key: "routing.http.response.access_control_allow_credentials.header_value",
        domain: AttributeDomain::Text,
        families: &[LoadBalancerFamily::Application],
        default: None,
    },
    ListenerAttributeSpec {
        key: "routing.http.response.access_control_expose_headers.header_value",
        domain: AttributeDomain::Text,
        families: &[LoadBalancerFamily::Application],
        default: None,
    },
    // "Specifies how long the results of a preflight request can be cached, in seconds."
    ListenerAttributeSpec {
        key: "routing.http.response.access_control_max_age.header_value",
        domain: AttributeDomain::Text,
        families: &[LoadBalancerFamily::Application],
        default: None,
    },
    ListenerAttributeSpec {
        key: "routing.http.response.content_security_policy.header_value",
        domain: AttributeDomain::Text,
        families: &[LoadBalancerFamily::Application],
        default: None,
    },
    ListenerAttributeSpec {
        key: "routing.http.response.x_content_type_options.header_value",
        domain: AttributeDomain::Text,
        families: &[LoadBalancerFamily::Application],
        default: None,
    },
    ListenerAttributeSpec {
        key: "routing.http.response.x_frame_options.header_value",
        domain: AttributeDomain::Text,
        families: &[LoadBalancerFamily::Application],
        default: None,
    },
];

/// One listener attribute: which listeners carry it, what it accepts, and the value it
/// reports until something sets it.
#[derive(Debug)]
pub struct ListenerAttributeSpec {
    pub key: &'static str,
    pub domain: AttributeDomain,
    /// The load balancer families whose listeners carry the attribute, as the model's
    /// `ListenerAttribute$Key` documentation groups them.
    pub families: &'static [LoadBalancerFamily],
    /// The documented default, or `None` for an attribute the model documents no default
    /// for — which is then reported only once a caller has set it.
    pub default: Option<&'static str>,
}

impl ListenerAttributeSpec {
    /// Whether a listener of a load balancer in `family` carries this attribute.
    pub fn applies_to(&self, family: LoadBalancerFamily) -> bool {
        self.families.contains(&family)
    }
}

/// The attribute `key` names, or `None` when no listener attribute carries that name.
pub fn listener_attribute(key: &str) -> Option<&'static ListenerAttributeSpec> {
    LISTENER_ATTRIBUTES.iter().find(|spec| spec.key == key)
}

/// Which family of load balancer a `Type` names, which is what decides WHICH listener
/// attributes its listeners carry. The pairing is the model's own `LoadBalancerTypeEnum`
/// (`application`, `network`, `gateway`), as [`load_balancer_type_code`] reads it.
pub fn load_balancer_family(lb_type: &str) -> LoadBalancerFamily {
    match lb_type {
        "network" => LoadBalancerFamily::Network,
        "gateway" => LoadBalancerFamily::Gateway,
        // `application` is the modelled default for `Type`.
        _ => LoadBalancerFamily::Application,
    }
}

/// The value `EnforceSecurityGroupInboundRulesOnPrivateLinkTraffic` has until a caller
/// sets it, from that member's own documentation: "The default is on."
pub const PRIVATE_LINK_INBOUND_RULES_DEFAULT: &str = "on";

/// One load balancer attribute: which load balancers carry it, what it accepts, the
/// value it reports until something sets it, and the value its documentation FIXES for
/// some load balancers.
pub struct LoadBalancerAttributeSpec {
    pub key: &'static str,
    pub domain: AttributeDomain,
    /// The load balancer families that carry the attribute, as the model's
    /// `LoadBalancerAttribute$Key` documentation groups them.
    families: &'static [LoadBalancerFamily],
    /// The documented default, or `None` for an attribute the model documents no default
    /// for — which is then reported only once a caller has set it.
    default: fn(&LoadBalancerRecord) -> Option<String>,
    /// The ONE value this attribute has on a load balancer whose documentation states it
    /// cannot be changed, or `None` where a caller may set it freely.
    fixed: fn(&LoadBalancerRecord) -> Option<String>,
}

impl std::fmt::Debug for LoadBalancerAttributeSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadBalancerAttributeSpec")
            .field("key", &self.key)
            .field("domain", &self.domain)
            .field("families", &self.families)
            .finish_non_exhaustive()
    }
}

impl LoadBalancerAttributeSpec {
    /// Whether `load_balancer` carries this attribute at all.
    pub fn applies_to(&self, load_balancer: &LoadBalancerRecord) -> bool {
        self.families.contains(&load_balancer.family())
    }

    /// The value `load_balancer` reports for this attribute until something sets it.
    pub fn default_for(&self, load_balancer: &LoadBalancerRecord) -> Option<String> {
        (self.default)(load_balancer)
    }

    /// The value this attribute is fixed at on `load_balancer`, or `None` when a caller
    /// may set it. A write of anything else is a configuration the service refuses.
    pub fn fixed_for(&self, load_balancer: &LoadBalancerRecord) -> Option<String> {
        (self.fixed)(load_balancer)
    }
}

/// Every load balancer attribute this service models, with the families that carry it,
/// the values it accepts and its documented default — each one a sentence of the model's
/// own `LoadBalancerAttribute$Key` documentation
/// (`smithy_query(query="doc",shape="LoadBalancerAttribute",member="Key")`), which is the
/// only catalog of them; the Smithy shape itself is an unconstrained key/value pair, so
/// nothing else in the model lists them.
pub static LOAD_BALANCER_ATTRIBUTES: &[LoadBalancerAttributeSpec] = &[
    // "The following attributes are supported by all load balancers:"
    //
    // "deletion_protection.enabled - Indicates whether deletion protection is enabled.
    // The value is true or false. The default is false."
    LoadBalancerAttributeSpec {
        key: "deletion_protection.enabled",
        domain: AttributeDomain::Boolean,
        families: EVERY_FAMILY,
        default: |_| Some("false".to_string()),
        fixed: |_| None,
    },
    // "load_balancing.cross_zone.enabled - ... The default for Network Load Balancers
    // and Gateway Load Balancers is false. The default for Application Load Balancers is
    // true, and can't be changed."
    LoadBalancerAttributeSpec {
        key: "load_balancing.cross_zone.enabled",
        domain: AttributeDomain::Boolean,
        families: EVERY_FAMILY,
        default: |load_balancer| {
            Some(
                match load_balancer.family() {
                    LoadBalancerFamily::Application => "true",
                    LoadBalancerFamily::Network | LoadBalancerFamily::Gateway => "false",
                }
                .to_string(),
            )
        },
        fixed: |load_balancer| {
            (load_balancer.family() == LoadBalancerFamily::Application)
                .then(|| "true".to_string())
        },
    },
    // "The following attributes are supported by both Application Load Balancers and
    // Network Load Balancers:"
    //
    // "access_logs.s3.enabled - Indicates whether access logs are enabled. The value is
    // true or false. The default is false."
    LoadBalancerAttributeSpec {
        key: "access_logs.s3.enabled",
        domain: AttributeDomain::Boolean,
        families: APPLICATION_AND_NETWORK,
        default: |_| Some("false".to_string()),
        fixed: |_| None,
    },
    // "access_logs.s3.bucket - The name of the S3 bucket for the access logs." A name
    // the caller chooses, with no domain and no default in the model.
    LoadBalancerAttributeSpec {
        key: "access_logs.s3.bucket",
        domain: AttributeDomain::Text,
        families: APPLICATION_AND_NETWORK,
        default: |_| None,
        fixed: |_| None,
    },
    // "access_logs.s3.prefix - The prefix for the location in the S3 bucket for the
    // access logs."
    LoadBalancerAttributeSpec {
        key: "access_logs.s3.prefix",
        domain: AttributeDomain::Text,
        families: APPLICATION_AND_NETWORK,
        default: |_| None,
        fixed: |_| None,
    },
    // "ipv6.deny_all_igw_traffic - Blocks internet gateway (IGW) access to the load
    // balancer. It is set to false for internet-facing load balancers and true for
    // internal load balancers".
    LoadBalancerAttributeSpec {
        key: "ipv6.deny_all_igw_traffic",
        domain: AttributeDomain::Boolean,
        families: APPLICATION_AND_NETWORK,
        default: |load_balancer| {
            Some(
                if load_balancer.scheme == INTERNAL_SCHEME {
                    "true"
                } else {
                    "false"
                }
                .to_string(),
            )
        },
        fixed: |_| None,
    },
    // "zonal_shift.config.enabled - Indicates whether zonal shift is enabled. The
    // possible values are true and false. The default is false."
    LoadBalancerAttributeSpec {
        key: "zonal_shift.config.enabled",
        domain: AttributeDomain::Boolean,
        families: APPLICATION_AND_NETWORK,
        default: |_| Some("false".to_string()),
        fixed: |_| None,
    },
    // "The following attributes are supported by only Application Load Balancers:"
    //
    // "idle_timeout.timeout_seconds - The idle timeout value, in seconds. The valid
    // range is 1-4000 seconds. The default is 60 seconds."
    LoadBalancerAttributeSpec {
        key: "idle_timeout.timeout_seconds",
        domain: AttributeDomain::Integer { min: 1, max: 4000 },
        families: APPLICATION_ONLY,
        default: |_| Some("60".to_string()),
        fixed: |_| None,
    },
    // "client_keep_alive.seconds - The client keep alive value, in seconds. The valid
    // range is 60-604800 seconds. The default is 3600 seconds."
    LoadBalancerAttributeSpec {
        key: "client_keep_alive.seconds",
        domain: AttributeDomain::Integer {
            min: 60,
            max: 604_800,
        },
        families: APPLICATION_ONLY,
        default: |_| Some("3600".to_string()),
        fixed: |_| None,
    },
    // "connection_logs.s3.enabled - Indicates whether connection logs are enabled. The
    // value is true or false. The default is false."
    LoadBalancerAttributeSpec {
        key: "connection_logs.s3.enabled",
        domain: AttributeDomain::Boolean,
        families: APPLICATION_ONLY,
        default: |_| Some("false".to_string()),
        fixed: |_| None,
    },
    // "connection_logs.s3.bucket - The name of the S3 bucket for the connection logs."
    LoadBalancerAttributeSpec {
        key: "connection_logs.s3.bucket",
        domain: AttributeDomain::Text,
        families: APPLICATION_ONLY,
        default: |_| None,
        fixed: |_| None,
    },
    // "connection_logs.s3.prefix - The prefix for the location in the S3 bucket for the
    // connection logs."
    LoadBalancerAttributeSpec {
        key: "connection_logs.s3.prefix",
        domain: AttributeDomain::Text,
        families: APPLICATION_ONLY,
        default: |_| None,
        fixed: |_| None,
    },
    // "health_check_logs.s3.enabled - Indicates whether health check logs are enabled.
    // The value is true or false. The default is false."
    LoadBalancerAttributeSpec {
        key: "health_check_logs.s3.enabled",
        domain: AttributeDomain::Boolean,
        families: APPLICATION_ONLY,
        default: |_| Some("false".to_string()),
        fixed: |_| None,
    },
    // "health_check_logs.s3.bucket - The name of the S3 bucket for the health check
    // logs."
    LoadBalancerAttributeSpec {
        key: "health_check_logs.s3.bucket",
        domain: AttributeDomain::Text,
        families: APPLICATION_ONLY,
        default: |_| None,
        fixed: |_| None,
    },
    // "health_check_logs.s3.prefix - The prefix for the location in the S3 bucket for
    // the health check logs."
    LoadBalancerAttributeSpec {
        key: "health_check_logs.s3.prefix",
        domain: AttributeDomain::Text,
        families: APPLICATION_ONLY,
        default: |_| None,
        fixed: |_| None,
    },
    // "routing.http.desync_mitigation_mode - ... The possible values are monitor,
    // defensive, and strictest. The default is defensive."
    LoadBalancerAttributeSpec {
        key: "routing.http.desync_mitigation_mode",
        domain: AttributeDomain::OneOf(&["monitor", "defensive", "strictest"]),
        families: APPLICATION_ONLY,
        default: |_| Some("defensive".to_string()),
        fixed: |_| None,
    },
    // "routing.http.drop_invalid_header_fields.enabled - ... The default is false."
    LoadBalancerAttributeSpec {
        key: "routing.http.drop_invalid_header_fields.enabled",
        domain: AttributeDomain::Boolean,
        families: APPLICATION_ONLY,
        default: |_| Some("false".to_string()),
        fixed: |_| None,
    },
    // "routing.http.preserve_host_header.enabled - ... The default is false."
    LoadBalancerAttributeSpec {
        key: "routing.http.preserve_host_header.enabled",
        domain: AttributeDomain::Boolean,
        families: APPLICATION_ONLY,
        default: |_| Some("false".to_string()),
        fixed: |_| None,
    },
    // "routing.http.x_amzn_tls_version_and_cipher_suite.enabled - ... The default is
    // false."
    LoadBalancerAttributeSpec {
        key: "routing.http.x_amzn_tls_version_and_cipher_suite.enabled",
        domain: AttributeDomain::Boolean,
        families: APPLICATION_ONLY,
        default: |_| Some("false".to_string()),
        fixed: |_| None,
    },
    // "routing.http.xff_client_port.enabled - ... The default is false."
    LoadBalancerAttributeSpec {
        key: "routing.http.xff_client_port.enabled",
        domain: AttributeDomain::Boolean,
        families: APPLICATION_ONLY,
        default: |_| Some("false".to_string()),
        fixed: |_| None,
    },
    // "routing.http.xff_header_processing.mode - ... The possible values are append,
    // preserve, and remove. The default is append."
    LoadBalancerAttributeSpec {
        key: "routing.http.xff_header_processing.mode",
        domain: AttributeDomain::OneOf(&["append", "preserve", "remove"]),
        families: APPLICATION_ONLY,
        default: |_| Some("append".to_string()),
        fixed: |_| None,
    },
    // "routing.http2.enabled - ... The default is true."
    LoadBalancerAttributeSpec {
        key: "routing.http2.enabled",
        domain: AttributeDomain::Boolean,
        families: APPLICATION_ONLY,
        default: |_| Some("true".to_string()),
        fixed: |_| None,
    },
    // "waf.fail_open.enabled - ... The default is false."
    LoadBalancerAttributeSpec {
        key: "waf.fail_open.enabled",
        domain: AttributeDomain::Boolean,
        families: APPLICATION_ONLY,
        default: |_| Some("false".to_string()),
        fixed: |_| None,
    },
    // "The following attributes are supported by only Network Load Balancers:"
    //
    // "dns_record.client_routing_policy - Indicates how traffic is distributed among the
    // load balancer Availability Zones. The possible values are
    // availability_zone_affinity ..., partial_availability_zone_affinity ... and
    // any_availability_zone ..." — three values and no documented default, so it is
    // reported once a caller has set one.
    LoadBalancerAttributeSpec {
        key: "dns_record.client_routing_policy",
        domain: AttributeDomain::OneOf(&[
            "availability_zone_affinity",
            "partial_availability_zone_affinity",
            "any_availability_zone",
        ]),
        families: NETWORK_ONLY,
        default: |_| None,
        fixed: |_| None,
    },
    // "secondary_ips.auto_assigned.per_subnet - The number of secondary IP addresses to
    // configure for your load balancer nodes. ... The valid range is 0 to 7. The default
    // is 0."
    LoadBalancerAttributeSpec {
        key: "secondary_ips.auto_assigned.per_subnet",
        domain: AttributeDomain::Integer { min: 0, max: 7 },
        families: NETWORK_ONLY,
        default: |_| Some("0".to_string()),
        fixed: |_| None,
    },
];

/// The families of an attribute the documentation lists under "supported by all load
/// balancers".
const EVERY_FAMILY: &[LoadBalancerFamily] = &[
    LoadBalancerFamily::Application,
    LoadBalancerFamily::Network,
    LoadBalancerFamily::Gateway,
];

/// The families of an attribute "supported by both Application Load Balancers and
/// Network Load Balancers".
const APPLICATION_AND_NETWORK: &[LoadBalancerFamily] =
    &[LoadBalancerFamily::Application, LoadBalancerFamily::Network];

/// The families of an attribute "supported by only Application Load Balancers".
const APPLICATION_ONLY: &[LoadBalancerFamily] = &[LoadBalancerFamily::Application];

/// The families of an attribute "supported by only Network Load Balancers".
const NETWORK_ONLY: &[LoadBalancerFamily] = &[LoadBalancerFamily::Network];

/// The attribute `key` names, or `None` when no load balancer attribute carries that
/// name.
pub fn load_balancer_attribute(key: &str) -> Option<&'static LoadBalancerAttributeSpec> {
    LOAD_BALANCER_ATTRIBUTES.iter().find(|spec| spec.key == key)
}

/// The `Scheme` of a load balancer that is not reachable from the internet, from the
/// model's `LoadBalancerSchemeEnum` (`internal`, `internet-facing`).
pub const INTERNAL_SCHEME: &str = "internal";

/// The one IP address type `IpAddressType` documents for every load balancer family.
pub const IPV4_ADDRESS_TYPE: &str = "ipv4";

/// The IP address type only an Application Load Balancer accepts, from
/// `SetIpAddressTypeInput$IpAddressType`: "[Application Load Balancers] The possible
/// values are ipv4 ..., dualstack ..., and dualstack-without-public-ipv4 ... [Network
/// Load Balancers and Gateway Load Balancers] The possible values are ipv4 ... and
/// dualstack".
pub const APPLICATION_ONLY_ADDRESS_TYPE: &str = "dualstack-without-public-ipv4";

/// Whether a load balancer of `family` with this `scheme` can use `ip_address_type`,
/// and — when it cannot — the rule it broke, so a refusal can name the value and the
/// rule the caller has to satisfy.
///
/// Two rules, both from `IpAddressType`'s own documentation on the operations that set
/// it: "Internal load balancers must use ipv4", and `dualstack-without-public-ipv4` is
/// bracketed for Application Load Balancers alone.
pub fn check_ip_address_type(
    family: LoadBalancerFamily,
    scheme: &str,
    ip_address_type: &str,
) -> Result<(), String> {
    if scheme == INTERNAL_SCHEME && ip_address_type != IPV4_ADDRESS_TYPE {
        return Err(format!(
            "The IP address type '{ip_address_type}' is not valid for an internal load \
             balancer, which must use '{IPV4_ADDRESS_TYPE}'"
        ));
    }
    if ip_address_type == APPLICATION_ONLY_ADDRESS_TYPE && family != LoadBalancerFamily::Application
    {
        return Err(format!(
            "The IP address type '{ip_address_type}' is only valid for an application load \
             balancer"
        ));
    }
    Ok(())
}

/// The letters AWS appends to a region to name an Availability Zone.
const ZONE_LETTERS: &[u8] = b"abcdefghijklmnopqrstuvwxyz";

/// How many subnets one load balancer is placed in at most.
///
/// `Subnets`/`SubnetMappings` are caller-chosen lists whose length the model bounds
/// nowhere, and the rule that bounds them is `Subnets`' own documentation: "you can
/// specify only one subnet per Availability Zone" — so the ceiling is the number of zones
/// a Region has. This emulator names those zones with the 26 letters [`ZONE_LETTERS`]
/// carries, far above the handful any AWS Region publishes, so that is the number: a
/// request past it cannot be one subnet per zone whatever its subnets are.
pub const SUBNETS_PER_LOAD_BALANCER: usize = ZONE_LETTERS.len();

/// The zones a load balancer placed in these subnets is attached to, one per requested
/// subnet, taken from `Subnets` or `SubnetMappings` (the model: "you must specify either
/// subnets or subnet mappings, but not both").
///
/// Which zone a subnet is in is EC2's to say, so it is the zone EC2 reported for that
/// subnet (`resolved`). A subnet EC2 could not be asked about — no EC2 is mounted, or it
/// holds no such subnet — is given a zone of the request's region that no other subnet of
/// this load balancer is in, so two subnets never read as the same zone whether their
/// zones were resolved or stood in for: the zones already taken are held in a set, and
/// the stand-in walks a cursor that never goes back, so every subnet is named in one
/// step. Either way the mapping is persisted on the load balancer, so every later read
/// reports the same zone for the same subnet.
///
/// The ONE reading of a request's subnets, called by `CreateLoadBalancer` for the zones a
/// new load balancer starts in and by `SetSubnets` for the zones that replace them. The
/// caller's list is bounded by [`SUBNETS_PER_LOAD_BALANCER`] before this runs.
pub fn availability_zone_records(
    subnets: &[String],
    subnet_mappings: &[SubnetMapping],
    region: &str,
    resolved: Option<&[network::SubnetFacts]>,
) -> Vec<AvailabilityZoneRecord> {
    let reported_zone = |subnet_id: &str| -> Option<String> {
        resolved?
            .iter()
            .find(|subnet| subnet.subnet_id == subnet_id)?
            .availability_zone
            .clone()
    };
    // Each requested subnet, in the order the request listed it, with the zone EC2
    // reported for it when it reported one.
    let mut requested: Vec<(String, Vec<LoadBalancerAddressRecord>, Option<String>)> = Vec::new();
    for subnet_id in subnets {
        let zone = reported_zone(subnet_id);
        requested.push((subnet_id.clone(), Vec::new(), zone));
    }
    for mapping in subnet_mappings {
        // Only what the caller pinned is kept: the emulator models no address pool,
        // so it never invents an `IpAddress`.
        let address = LoadBalancerAddressRecord {
            allocation_id: mapping.allocation_id.clone(),
            private_ipv4_address: mapping.private_i_pv4_address.clone(),
            ipv6_address: mapping.i_pv6_address.clone(),
        };
        let addresses = if address.is_empty() {
            Vec::new()
        } else {
            vec![address]
        };
        let subnet_id = mapping.subnet_id.clone().unwrap_or_default();
        let zone = reported_zone(&subnet_id);
        requested.push((subnet_id, addresses, zone));
    }

    let mut taken: std::collections::HashSet<String> = requested
        .iter()
        .filter_map(|(_, _, zone)| zone.clone())
        .collect();
    let mut cursor = 0usize;
    requested
        .into_iter()
        .map(|(subnet_id, addresses, zone)| {
            let zone_name = zone.unwrap_or_else(|| loop {
                let letter = char::from(ZONE_LETTERS[cursor % ZONE_LETTERS.len()]);
                let wrap = cursor / ZONE_LETTERS.len();
                cursor += 1;
                let candidate = if wrap == 0 {
                    format!("{region}{letter}")
                } else {
                    format!("{region}{letter}{wrap}")
                };
                if taken.insert(candidate.clone()) {
                    return candidate;
                }
            });
            AvailabilityZoneRecord {
                zone_name,
                subnet_id,
                outpost_id: None,
                addresses,
            }
        })
        .collect()
}

/// The message Elastic Load Balancing answers a `ResourceNotFound` with for a policy read
/// whose `ResourceArn` names no policy to report.
///
/// Built from the request: the ARN it names is the only thing the caller can act on, and
/// what the caller has to fix is that ARN — whether it named a trust store this account
/// does not have, one no policy is attached to, or a resource that carries no resource
/// policy at all.
pub fn trust_store_policy_not_found_message(arn: &str) -> String {
    format!("Trust store ARN '{arn}' not found")
}

/// The message Elastic Load Balancing answers a `LoadBalancerNotFound` with for an
/// operation that addresses ONE load balancer through its own required
/// `LoadBalancerArn` — the attribute read and write, the network writes
/// (`SetIpAddressType`, `SetSecurityGroups`, `SetSubnets`, `ModifyIpPools`), the
/// capacity-reservation read and write, and the creates and reads that hang a resource
/// off one.
///
/// Built from the request: the identifier it names is the only thing the caller can act
/// on, exactly as the listener-addressing operations quote their `ListenerArn` back
/// ([`listener_not_found_message`]), while a read over a LIST of load balancers names
/// either the identifiers it could not resolve ([`load_balancers_not_found_message`]) or
/// no value at all ([`load_balancer_selection_not_found_message`]).
pub fn load_balancer_not_found_message(arn: &str) -> String {
    format!("Load balancer '{arn}' not found")
}

/// The message Elastic Load Balancing answers a `LoadBalancerNotFound` with for a read
/// over a LIST of identifiers (`DescribeLoadBalancers`, whose `LoadBalancerArns` and
/// `Names` are both lists).
///
/// Built from the request, like every other not-found message here: the identifiers that
/// did not resolve, under the PLURAL noun, rendered as the LIST the request carried —
/// `Load balancers '[my-alb]' not found` for one name, and the same form with more
/// entries for more of them. The brackets are what makes it a list rather than one
/// resource, which is what the recorded answer to a `DescribeLoadBalancers(Names=[...])`
/// of an absent name carries.
pub fn load_balancers_not_found_message<'a>(
    identifiers: impl IntoIterator<Item = &'a str>,
) -> String {
    let named: Vec<&str> = identifiers.into_iter().collect();
    format!("Load balancers '[{}]' not found", named.join(", "))
}

/// The message Elastic Load Balancing answers a `LoadBalancerNotFound` with for a read
/// that selected by ARN (`DescribeLoadBalancers`' `LoadBalancerArns`).
///
/// The same read's two selectors are refused in different words: a selection by NAME
/// lists the names that did not resolve ([`load_balancers_not_found_message`]), while a
/// selection by ARN names no value at all — the shape of message this service's sibling
/// list reads answer ([`listener_selection_not_found_message`],
/// [`target_group_selection_not_found_message`]), and what the recorded answer to a
/// `DescribeLoadBalancers(LoadBalancerArns=[...])` of an absent ARN carries.
pub fn load_balancer_selection_not_found_message() -> String {
    "One or more load balancers not found".to_string()
}

/// Whether a protocol carries HTTP semantics, which is what decides whether a health
/// check has a path and a response-code matcher and whether a target group reports a
/// `ProtocolVersion`.
pub fn is_http_protocol(protocol: Option<&str>) -> bool {
    matches!(protocol, Some("HTTP") | Some("HTTPS"))
}

/// The protocols a HEALTH CHECK may use, from `HealthCheckProtocol`'s own
/// documentation: "The GENEVE, TLS, UDP, TCP_UDP, QUIC, and TCP_QUIC protocols are not
/// supported for health checks" — leaving HTTP, HTTPS and TCP.
pub const HEALTH_CHECK_PROTOCOLS: &[&str] = &["HTTP", "HTTPS", "TCP"];

/// The one `HealthCheckPort` value that is not a port number: the model documents it as
/// that member's default — "traffic-port, which is the port on which each target
/// receives traffic from the load balancer".
pub const TRAFFIC_PORT: &str = "traffic-port";

/// The ports a health check may name. `HealthCheckPort` is modelled as a bare string
/// (`smithy_query(query="shape",shape="HealthCheckPort")` -> `{"type": "string"}`), so
/// the model constrains nothing about it and the handler that accepts it is the only
/// gate; the bound is the one the model puts on every port member it does type as an
/// integer (`Port` and `TargetControlPort`: `range 1..65535`).
pub const HEALTH_CHECK_PORT_RANGE: std::ops::RangeInclusive<u32> = 1..=65535;

/// Whether `value` is a health-check port this service accepts — the literal
/// [`TRAFFIC_PORT`], or a port inside [`HEALTH_CHECK_PORT_RANGE`] — and, when it is
/// not, the rule it broke, so the refusal can name the member, the value and the
/// bound the caller has to fix.
pub fn check_health_check_port(value: &str) -> Result<(), String> {
    if value == TRAFFIC_PORT
        || value
            .parse::<u32>()
            .is_ok_and(|port| HEALTH_CHECK_PORT_RANGE.contains(&port))
    {
        return Ok(());
    }
    Err(format!(
        "Health check port '{value}' must be within '{}-{}' or '{TRAFFIC_PORT}'",
        HEALTH_CHECK_PORT_RANGE.start(),
        HEALTH_CHECK_PORT_RANGE.end(),
    ))
}

/// The published maximum length of a target group name, from `CreateTargetGroup`'s
/// `Name` documentation: "can have a maximum of 32 characters".
pub const TARGET_GROUP_NAME_MAX_LENGTH: usize = 32;

/// The maximum length of a health-check path, from the `@length` the model puts on the
/// `Path` shape `HealthCheckPath` targets (`smithy_query(query="shape",shape="Path")` ->
/// `length 1..1024`).
pub const HEALTH_CHECK_PATH_MAX_LENGTH: usize = 1024;

/// Whether `value` is a health-check path this service accepts — a destination the load
/// balancer can request: an absolute path, so it begins with `/`, written in printable
/// ASCII with no spaces, within the length the model bounds it to — and, when it is not,
/// the rule it broke, so the refusal can name the member, the value and the rule the
/// caller has to fix.
///
/// The model types `HealthCheckPath` as `Path` with a length alone, so every other clause
/// is server-side and this is the only gate: a path the load balancer cannot send is a
/// probe that can never match, which is why it is refused at the create rather than
/// stored.
pub fn check_health_check_path(value: &str) -> Result<(), String> {
    let printable_ascii_without_spaces = value
        .chars()
        .all(|character| character.is_ascii_graphic());
    // The character rule first, so a value that is no absolute printable-ASCII path is
    // told that rather than a length it may well be inside.
    if !value.starts_with('/') || !printable_ascii_without_spaces {
        return Err(format!(
            "Health check path '{value}' must begin with a '/' character and can only \
             contain printable ASCII characters, without spaces"
        ));
    }
    // The model's `@length` counts CHARACTERS, not bytes.
    if value.chars().count() > HEALTH_CHECK_PATH_MAX_LENGTH {
        return Err(format!(
            "Health check path '{value}' cannot be longer than \
             '{HEALTH_CHECK_PATH_MAX_LENGTH}' characters"
        ));
    }
    Ok(())
}

/// Which family of load balancer a target group serves, which is what decides WHICH
/// attributes it carries (the families the model's `TargetGroupAttribute$Key`
/// documentation groups the attributes by) and what its health check defaults to.
///
/// Derived from the target group's own protocol, as the model's `Protocol`
/// documentation pairs them: "For Application Load Balancers, the supported protocols
/// are HTTP and HTTPS. For Network Load Balancers, the supported protocols are TCP,
/// TLS, UDP, TCP_UDP, QUIC, or TCP_QUIC. For Gateway Load Balancers, the supported
/// protocol is GENEVE." A `lambda` target group has no protocol at all and is an
/// Application Load Balancer's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadBalancerFamily {
    Application,
    Network,
    Gateway,
}

/// The family a target group whose protocol is `protocol` serves — see
/// [`LoadBalancerFamily`]. The ONE reading of that pairing, which
/// [`TargetGroupRecord::family`] answers from the stored record and a create calls
/// before there is a record to ask.
pub fn protocol_family(protocol: Option<&str>) -> LoadBalancerFamily {
    match protocol {
        Some("GENEVE") => LoadBalancerFamily::Gateway,
        Some("TCP") | Some("TLS") | Some("UDP") | Some("TCP_UDP") | Some("QUIC")
        | Some("TCP_QUIC") => LoadBalancerFamily::Network,
        // HTTP, HTTPS, and a `lambda` target group, which carries no protocol.
        _ => LoadBalancerFamily::Application,
    }
}

/// The set of values one attribute accepts, as its own documentation states it
/// (`smithy_query(query="doc",shape="TargetGroupAttribute",member="Key")` for a target
/// group's, `shape="ListenerAttribute"` for a listener's).
#[derive(Debug, Clone, Copy)]
pub enum AttributeDomain {
    /// "The value is `true` or `false`."
    Boolean,
    /// Any string: what the documentation describes as a name or a header value the
    /// caller chooses, stating no set of values to check it against.
    Text,
    /// An integer in the documented inclusive range.
    Integer { min: i64, max: i64 },
    /// The literal `off`, or an integer in the documented inclusive range.
    IntegerOrOff { min: i64, max: i64 },
    /// `0` (the documented "disabled"), or an integer in the documented range.
    ZeroOrInteger { min: i64, max: i64 },
    /// One of a documented set of literals.
    OneOf(&'static [&'static str]),
    /// A cookie name: any value whose prefix is not one the load balancer reserves.
    CookieName,
}

/// The cookie-name prefixes `stickiness.app_cookie.cookie_name` may not carry:
/// "Names that start with the following prefixes are not allowed: AWSALB, AWSALBAPP,
/// and AWSALBTG; they're reserved for use by the load balancer."
const RESERVED_COOKIE_PREFIXES: &[&str] = &["AWSALB", "AWSALBAPP", "AWSALBTG"];

impl AttributeDomain {
    /// Whether `value` is in this domain, and — when it is not — the domain stated the
    /// way the attribute's documentation states it, so a refusal can name it.
    pub fn check(&self, value: &str) -> Result<(), String> {
        let integer = |min: i64, max: i64| -> Result<(), String> {
            match value.parse::<i64>() {
                Ok(parsed) if parsed >= min && parsed <= max => Ok(()),
                _ => Err(format!("an integer from {min} to {max}")),
            }
        };
        match *self {
            Self::Boolean => match value {
                "true" | "false" => Ok(()),
                _ => Err("'true' or 'false'".to_string()),
            },
            // Nothing to check: the documentation states no domain for it, so refusing
            // a value here would be this emulator's own rule rather than the service's.
            Self::Text => Ok(()),
            Self::Integer { min, max } => integer(min, max),
            Self::IntegerOrOff { min, max } => {
                if value == "off" {
                    return Ok(());
                }
                integer(min, max).map_err(|domain| format!("'off' or {domain}"))
            }
            Self::ZeroOrInteger { min, max } => {
                if value == "0" {
                    return Ok(());
                }
                integer(min, max).map_err(|domain| format!("0 or {domain}"))
            }
            Self::OneOf(values) => {
                if values.contains(&value) {
                    return Ok(());
                }
                Err(format!("one of {}", values.join(", ")))
            }
            Self::CookieName => {
                if RESERVED_COOKIE_PREFIXES
                    .iter()
                    .any(|prefix| value.starts_with(prefix))
                {
                    return Err(format!(
                        "a name not starting with {}",
                        RESERVED_COOKIE_PREFIXES.join(", ")
                    ));
                }
                Ok(())
            }
        }
    }
}

/// One target group attribute: which target groups carry it, what it accepts, and the
/// value it reports until something sets it.
pub struct AttributeSpec {
    pub key: &'static str,
    pub domain: AttributeDomain,
    /// Whether a target group of this shape carries the attribute at all.
    applies: fn(&TargetGroupRecord) -> bool,
    /// The documented default, or `None` for an attribute the model documents no
    /// default for — which is then reported only once a caller has set it.
    default: fn(&TargetGroupRecord) -> Option<String>,
}

impl std::fmt::Debug for AttributeSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttributeSpec")
            .field("key", &self.key)
            .field("domain", &self.domain)
            .finish_non_exhaustive()
    }
}

impl AttributeSpec {
    /// Whether `group` carries this attribute.
    pub fn applies_to(&self, group: &TargetGroupRecord) -> bool {
        (self.applies)(group)
    }

    /// The value `group` reports for this attribute until something sets it.
    pub fn default_for(&self, group: &TargetGroupRecord) -> Option<String> {
        (self.default)(group)
    }
}

/// Every target group attribute this service models, with the family it belongs to,
/// the values it accepts and its default — each one a sentence of the model's own
/// `TargetGroupAttribute$Key` documentation
/// (`smithy_query(query="doc",shape="TargetGroupAttribute",member="Key")`), which is
/// the only catalog of them; the Smithy shape itself is an unconstrained key/value
/// pair, so nothing else in the model lists them.
pub static TARGET_GROUP_ATTRIBUTES: &[AttributeSpec] = &[
    // Supported by all load balancers.
    AttributeSpec {
        key: "deregistration_delay.timeout_seconds",
        domain: AttributeDomain::Integer { min: 0, max: 3600 },
        // "If the target is a Lambda function, this attribute is not supported."
        applies: |group| !group.is_lambda(),
        default: |_| Some("300".to_string()),
    },
    AttributeSpec {
        key: "stickiness.enabled",
        domain: AttributeDomain::Boolean,
        applies: |group| !group.is_lambda(),
        default: |_| Some("false".to_string()),
    },
    AttributeSpec {
        key: "stickiness.type",
        domain: AttributeDomain::OneOf(&[
            "lb_cookie",
            "app_cookie",
            "source_ip",
            "source_ip_dest_ip",
            "source_ip_dest_ip_proto",
        ]),
        applies: |group| !group.is_lambda(),
        // "lb_cookie and app_cookie for Application Load Balancers. source_ip for
        // Network Load Balancers. source_ip_dest_ip and source_ip_dest_ip_proto for
        // Gateway Load Balancers."
        default: |group| {
            Some(
                match group.family() {
                    LoadBalancerFamily::Application => "lb_cookie",
                    LoadBalancerFamily::Network => "source_ip",
                    LoadBalancerFamily::Gateway => "source_ip_dest_ip",
                }
                .to_string(),
            )
        },
    },
    // Supported by Application Load Balancers and Network Load Balancers — and so by
    // neither a Gateway Load Balancer's target group nor a Lambda one (see
    // [`TargetGroupRecord::carries_connection_attributes`]).
    AttributeSpec {
        key: "load_balancing.cross_zone.enabled",
        domain: AttributeDomain::OneOf(&["true", "false", "use_load_balancer_configuration"]),
        applies: TargetGroupRecord::carries_connection_attributes,
        default: |_| Some("use_load_balancer_configuration".to_string()),
    },
    AttributeSpec {
        key: "target_group_health.dns_failover.minimum_healthy_targets.count",
        domain: AttributeDomain::IntegerOrOff { min: 1, max: i64::MAX },
        applies: TargetGroupRecord::carries_connection_attributes,
        default: |_| Some("1".to_string()),
    },
    AttributeSpec {
        key: "target_group_health.dns_failover.minimum_healthy_targets.percentage",
        domain: AttributeDomain::IntegerOrOff { min: 1, max: 100 },
        applies: TargetGroupRecord::carries_connection_attributes,
        default: |_| Some("off".to_string()),
    },
    AttributeSpec {
        key: "target_group_health.unhealthy_state_routing.minimum_healthy_targets.count",
        domain: AttributeDomain::Integer { min: 1, max: i64::MAX },
        applies: TargetGroupRecord::carries_connection_attributes,
        default: |_| Some("1".to_string()),
    },
    AttributeSpec {
        key: "target_group_health.unhealthy_state_routing.minimum_healthy_targets.percentage",
        domain: AttributeDomain::IntegerOrOff { min: 1, max: 100 },
        applies: TargetGroupRecord::carries_connection_attributes,
        default: |_| Some("off".to_string()),
    },
    // Supported only if the load balancer is an Application Load Balancer and the
    // target is an instance or an IP address.
    AttributeSpec {
        key: "load_balancing.algorithm.type",
        domain: AttributeDomain::OneOf(&[
            "round_robin",
            "least_outstanding_requests",
            "weighted_random",
        ]),
        applies: TargetGroupRecord::is_application_with_addressable_targets,
        default: |_| Some("round_robin".to_string()),
    },
    AttributeSpec {
        key: "load_balancing.algorithm.anomaly_mitigation",
        domain: AttributeDomain::OneOf(&["on", "off"]),
        applies: TargetGroupRecord::is_application_with_addressable_targets,
        default: |_| Some("off".to_string()),
    },
    AttributeSpec {
        key: "slow_start.duration_seconds",
        // "The range is 30-900 seconds (15 minutes). The default is 0 seconds
        // (disabled)."
        domain: AttributeDomain::ZeroOrInteger { min: 30, max: 900 },
        applies: TargetGroupRecord::is_application_with_addressable_targets,
        default: |_| Some("0".to_string()),
    },
    AttributeSpec {
        key: "stickiness.app_cookie.cookie_name",
        domain: AttributeDomain::CookieName,
        applies: TargetGroupRecord::is_application_with_addressable_targets,
        // The model documents no default: the attribute is reported once set.
        default: |_| None,
    },
    AttributeSpec {
        key: "stickiness.app_cookie.duration_seconds",
        domain: AttributeDomain::Integer { min: 1, max: 604_800 },
        applies: TargetGroupRecord::is_application_with_addressable_targets,
        default: |_| Some("86400".to_string()),
    },
    AttributeSpec {
        key: "stickiness.lb_cookie.duration_seconds",
        domain: AttributeDomain::Integer { min: 1, max: 604_800 },
        applies: TargetGroupRecord::is_application_with_addressable_targets,
        default: |_| Some("86400".to_string()),
    },
    // Supported only if the load balancer is an Application Load Balancer and the
    // target is a Lambda function.
    AttributeSpec {
        key: "lambda.multi_value_headers.enabled",
        domain: AttributeDomain::Boolean,
        applies: TargetGroupRecord::is_lambda,
        default: |_| Some("false".to_string()),
    },
    // Supported only by Network Load Balancers.
    AttributeSpec {
        key: "deregistration_delay.connection_termination.enabled",
        domain: AttributeDomain::Boolean,
        applies: |group| group.family() == LoadBalancerFamily::Network,
        // "For new UDP/TCP_UDP target groups the default is true. Otherwise, the
        // default is false."
        default: |group| {
            Some(
                match group.protocol.as_deref() {
                    Some("UDP") | Some("TCP_UDP") => "true",
                    _ => "false",
                }
                .to_string(),
            )
        },
    },
    AttributeSpec {
        key: "preserve_client_ip.enabled",
        domain: AttributeDomain::Boolean,
        applies: |group| group.family() == LoadBalancerFamily::Network,
        // "The default is disabled if the target group type is IP address and the
        // target group protocol is TCP or TLS. Otherwise, the default is enabled."
        default: |group| {
            let ip_tcp = group.target_type == "ip"
                && matches!(group.protocol.as_deref(), Some("TCP") | Some("TLS"));
            Some(if ip_tcp { "false" } else { "true" }.to_string())
        },
    },
    AttributeSpec {
        key: "proxy_protocol_v2.enabled",
        domain: AttributeDomain::Boolean,
        applies: |group| group.family() == LoadBalancerFamily::Network,
        default: |_| Some("false".to_string()),
    },
    AttributeSpec {
        key: "target_health_state.unhealthy.connection_termination.enabled",
        domain: AttributeDomain::Boolean,
        applies: |group| group.family() == LoadBalancerFamily::Network,
        default: |_| Some("true".to_string()),
    },
    AttributeSpec {
        key: "target_health_state.unhealthy.draining_interval_seconds",
        domain: AttributeDomain::Integer { min: 0, max: 360_000 },
        applies: |group| group.family() == LoadBalancerFamily::Network,
        default: |_| Some("0".to_string()),
    },
    // Supported only by Gateway Load Balancers.
    AttributeSpec {
        key: "target_failover.on_deregistration",
        domain: AttributeDomain::OneOf(&["rebalance", "no_rebalance"]),
        applies: |group| group.family() == LoadBalancerFamily::Gateway,
        default: |_| Some("no_rebalance".to_string()),
    },
    AttributeSpec {
        key: "target_failover.on_unhealthy",
        domain: AttributeDomain::OneOf(&["rebalance", "no_rebalance"]),
        applies: |group| group.family() == LoadBalancerFamily::Gateway,
        default: |_| Some("no_rebalance".to_string()),
    },
    AttributeSpec {
        key: "send_tcp_reset.on_unhealthy.enabled",
        domain: AttributeDomain::Boolean,
        applies: |group| group.family() == LoadBalancerFamily::Gateway,
        default: |_| Some("false".to_string()),
    },
    AttributeSpec {
        key: "send_tcp_reset.on_deregistration.enabled",
        domain: AttributeDomain::Boolean,
        applies: |group| group.family() == LoadBalancerFamily::Gateway,
        default: |_| Some("false".to_string()),
    },
];

/// The attribute `key` names, or `None` when no target group attribute carries that
/// name.
pub fn target_group_attribute(key: &str) -> Option<&'static AttributeSpec> {
    TARGET_GROUP_ATTRIBUTES.iter().find(|spec| spec.key == key)
}

/// One target registered with a target group.
#[derive(Debug, Clone)]
pub struct RegisteredTarget {
    pub id: String,
    /// The port the target receives traffic on: the one `RegisterTargets` gave, else
    /// the target group's own port.
    pub port: Option<i32>,
    pub availability_zone: Option<String>,
    pub registered_at: SystemTime,
    /// When the clock owes this registration its move out of `initial`.
    pub healthy_at: SystemTime,
}

/// A persisted target group.
#[derive(Debug, Clone, Default)]
pub struct TargetGroupRecord {
    pub arn: String,
    pub name: String,
    pub protocol: Option<String>,
    pub protocol_version: Option<String>,
    pub port: Option<i32>,
    pub vpc_id: Option<String>,
    pub target_type: String,
    pub ip_address_type: Option<String>,
    pub target_control_port: Option<i32>,
    pub health_check: HealthCheckConfig,
    /// The load balancers whose listeners (or rules) forward to this target group.
    pub load_balancer_arns: Vec<String>,
    pub attributes: HashMap<String, String>,
    pub tags: HashMap<String, String>,
    /// Registrations keyed by target id and effective port, so registering the same
    /// id on two ports keeps two registrations and re-registering one replaces it.
    pub targets: BTreeMap<(String, Option<i32>), RegisteredTarget>,
}

impl TargetGroupRecord {
    /// Whether this target group's targets are Lambda functions, which is what several
    /// defaults and one whole attribute family key on.
    pub fn is_lambda(&self) -> bool {
        self.target_type == "lambda"
    }

    /// Which load balancer family this target group serves — see
    /// [`LoadBalancerFamily`].
    pub fn family(&self) -> LoadBalancerFamily {
        protocol_family(self.protocol.as_deref())
    }

    /// Whether the attributes documented "only if the load balancer is an Application
    /// Load Balancer and the target is an instance or an IP address" apply here.
    pub fn is_application_with_addressable_targets(&self) -> bool {
        self.family() == LoadBalancerFamily::Application
            && matches!(self.target_type.as_str(), "instance" | "ip")
    }

    /// Whether the attributes that describe how the load balancer CONNECTS to this
    /// group's targets — stickiness, cross-zone load balancing, the target-group-health
    /// thresholds — apply here.
    ///
    /// They are documented for an Application or a Network Load Balancer, which rules
    /// out a Gateway Load Balancer's group; and they presume a target the load balancer
    /// opens connections to, which a Lambda function is not. The model documents
    /// exactly ONE attribute for a Lambda target
    /// (`lambda.multi_value_headers.enabled`), and the recorded
    /// `DescribeTargetGroupAttributes` of a Lambda target group answers a set this
    /// service matched only once the rest were left out of it.
    pub fn carries_connection_attributes(&self) -> bool {
        self.family() != LoadBalancerFamily::Gateway && !self.is_lambda()
    }

    /// Whether this target group's health check has a path and a response-code matcher.
    ///
    /// The model binds both to an HTTP/HTTPS health check ("[HTTP/HTTPS health checks]
    /// The destination for health checks on the targets"). A `lambda` target group
    /// reports no health-check protocol at all — the load balancer invokes the function
    /// rather than connecting to a port — and its health check is an HTTP one, so it
    /// carries them too.
    pub fn health_check_is_http(&self) -> bool {
        self.is_lambda() || is_http_protocol(self.health_check.protocol.as_deref())
    }

    /// Materialize the health-check members that exist only for an HTTP-semantic health
    /// check: fill the documented default of each one nothing has set, and drop both
    /// when the health check is not an HTTP one (a TCP check has neither a path nor a
    /// response-code matcher to report).
    ///
    /// Called after a create or a modify resolves the health check, so the stored
    /// record \u2014 and therefore every later read of it \u2014 carries exactly the members AWS
    /// reports for a group of this shape.
    pub fn settle_health_check_http_members(&mut self) {
        if self.health_check_is_http() {
            let protocol_version = self.protocol_version.clone();
            if self.health_check.path.is_none() {
                self.health_check.path = Some(default_health_check_path(protocol_version.as_deref()));
            }
            if self.health_check.matcher.is_none() {
                self.health_check.matcher = Some(default_health_check_matcher(
                    protocol_version.as_deref(),
                    self.family(),
                ));
            }
        } else {
            self.health_check.path = None;
            self.health_check.matcher = None;
        }
    }

    /// Every attribute this target group reports: each one the model documents for a
    /// group of this shape, carrying what a caller set or, failing that, its documented
    /// default. Keyed in a stable order so two reads of one target group answer alike.
    pub fn effective_attributes(&self) -> Vec<(String, String)> {
        let mut reported: Vec<(String, String)> = TARGET_GROUP_ATTRIBUTES
            .iter()
            .filter(|spec| spec.applies_to(self))
            .filter_map(|spec| {
                let value = self
                    .attributes
                    .get(spec.key)
                    .cloned()
                    .or_else(|| spec.default_for(self))?;
                Some((spec.key.to_string(), value))
            })
            .collect();
        reported.sort_by(|left, right| left.0.cmp(&right.0));
        reported
    }

    /// [`Self::effective_attributes`] as the modelled shape.
    pub fn attribute_shapes(&self) -> Vec<TargetGroupAttribute> {
        self.effective_attributes()
            .into_iter()
            .map(|(key, value)| TargetGroupAttribute {
                key: Some(key),
                value: Some(value),
            })
            .collect()
    }

    /// The settings `CreateTargetGroup` is idempotent over — see
    /// [`LoadBalancerRecord::has_settings_of`].
    pub fn has_settings_of(&self, other: &TargetGroupRecord) -> bool {
        self.protocol == other.protocol
            && self.protocol_version == other.protocol_version
            && self.port == other.port
            && self.vpc_id == other.vpc_id
            && self.target_type == other.target_type
            && self.ip_address_type == other.ip_address_type
            && self.target_control_port == other.target_control_port
            && self.health_check == other.health_check
            && self.tags == other.tags
    }

    pub fn to_shape(&self) -> TargetGroup {
        TargetGroup {
            target_group_arn: Some(self.arn.clone()),
            target_group_name: Some(self.name.clone()),
            protocol: self.protocol.clone(),
            port: self.port,
            vpc_id: self.vpc_id.clone(),
            health_check_protocol: self.health_check.protocol.clone(),
            health_check_port: self.health_check.port.clone(),
            health_check_enabled: Some(self.health_check.enabled),
            health_check_interval_seconds: Some(self.health_check.interval_seconds),
            health_check_timeout_seconds: Some(self.health_check.timeout_seconds),
            healthy_threshold_count: Some(self.health_check.healthy_threshold),
            unhealthy_threshold_count: Some(self.health_check.unhealthy_threshold),
            health_check_path: self.health_check.path.clone(),
            matcher: self.health_check.matcher.as_ref().map(MatcherRecord::to_shape),
            load_balancer_arns: Some(self.load_balancer_arns.clone()),
            target_type: Some(self.target_type.clone()),
            protocol_version: self.protocol_version.clone(),
            ip_address_type: self.ip_address_type.clone(),
            target_control_port: self.target_control_port,
        }
    }

    /// The port a health check of `target` runs against: the configured health-check
    /// port, or — for the modelled default [`TRAFFIC_PORT`] — the port the target
    /// receives traffic on.
    pub fn health_check_port_of(&self, target: &RegisteredTarget) -> Option<String> {
        match self.health_check.port.as_deref() {
            Some(TRAFFIC_PORT) | None => target.port.or(self.port).map(|p| p.to_string()),
            Some(port) => Some(port.to_string()),
        }
    }

    /// The health of one registered target, derived from what is stored plus the
    /// clock: a target group no load balancer forwards to has nothing checking its
    /// targets, a group whose health checks are off reports them unavailable, and a
    /// registration younger than [`SETTLE_AFTER`] is still coming up.
    pub fn health_of(&self, target: &RegisteredTarget, now: SystemTime) -> TargetHealth {
        if self.load_balancer_arns.is_empty() {
            return TargetHealth {
                state: Some("unused".to_string()),
                reason: Some("Target.NotInUse".to_string()),
                description: Some(
                    "Target group is not configured to receive traffic from the load balancer"
                        .to_string(),
                ),
            };
        }
        if !self.health_check.enabled {
            return TargetHealth {
                state: Some("unavailable".to_string()),
                reason: Some("Target.HealthCheckDisabled".to_string()),
                description: Some("Health checks are not enabled for this target group".to_string()),
            };
        }
        if now < target.healthy_at {
            return TargetHealth {
                state: Some("initial".to_string()),
                reason: Some("Elb.RegistrationInProgress".to_string()),
                description: Some("Target registration is in progress".to_string()),
            };
        }
        TargetHealth {
            state: Some("healthy".to_string()),
            reason: None,
            description: None,
        }
    }

    /// The description of a target that is not registered with this group.
    pub fn unregistered_health_description(&self, target: &TargetDescription) -> TargetHealthDescription {
        TargetHealthDescription {
            target: Some(target.clone()),
            health_check_port: None,
            target_health: Some(TargetHealth {
                state: Some("unused".to_string()),
                reason: Some("Target.NotRegistered".to_string()),
                description: Some("Target is not registered to the target group".to_string()),
            }),
            anomaly_detection: None,
            administrative_override: None,
        }
    }

    /// The health description of one registered target.
    pub fn health_description_of(
        &self,
        target: &RegisteredTarget,
        now: SystemTime,
    ) -> TargetHealthDescription {
        TargetHealthDescription {
            target: Some(TargetDescription {
                id: target.id.clone(),
                port: target.port,
                availability_zone: target.availability_zone.clone(),
                quic_server_id: None,
            }),
            health_check_port: self.health_check_port_of(target),
            target_health: Some(self.health_of(target, now)),
            anomaly_detection: None,
            administrative_override: None,
        }
    }
}

/// The statuses the model's `TrustStoreStatus` enum names
/// (`smithy_query(query="shape",shape="TrustStoreStatus")`: `ACTIVE`, `CREATING`).
/// A trust store AWS returns from `CreateTrustStore` is not ready yet, so it starts
/// at `CREATING` and the clock owes it [`TRUST_STORE_ACTIVE`] at [`SETTLE_AFTER`].
pub const TRUST_STORE_CREATING: &str = "CREATING";
/// The status of a trust store whose settling span is over.
pub const TRUST_STORE_ACTIVE: &str = "ACTIVE";

/// A persisted trust store: everything `DescribeTrustStores` reports about one, plus
/// the two instants that decide the order it is reported in and when the clock owes it
/// its move to [`TRUST_STORE_ACTIVE`].
///
/// `CreateTrustStore` — the operation that mints one, counting
/// `number_of_ca_certificates` from the CA certificates bundle it is given — is still
/// a scaffold stub with its own issue; this is the record it fills, and the one
/// `DescribeTrustStores` reads.
#[derive(Debug, Clone)]
pub struct TrustStoreRecord {
    pub arn: String,
    pub name: String,
    pub status: String,
    /// When the clock owes this trust store its move to [`TRUST_STORE_ACTIVE`].
    pub active_at: SystemTime,
    /// Creation order, which is the order a paginated read walks it in. Not reported:
    /// the `TrustStore` shape models no creation time.
    pub created_at: SystemTime,
    /// How many CA certificates the trust store's bundle holds — counted from that
    /// bundle's bytes when it is stored, never guessed.
    pub number_of_ca_certificates: i32,
    /// How many revoked certificates the trust store's revocation lists hold. Written
    /// by `AddTrustStoreRevocations`/`RemoveTrustStoreRevocations`, both still scaffold
    /// stubs, so the shape is read off this field rather than restated: it follows the
    /// store the day a revocation can be added.
    pub total_revoked_entries: i64,
    /// The tags on this trust store, which `DescribeTags` reports. A trust store is a
    /// taggable Elastic Load Balancing resource — `AddTags`/`RemoveTags` both model
    /// `TrustStoreNotFoundException` — so the tags live on the record beside every
    /// other taggable resource's, and a read answers what is stored rather than
    /// assuming a trust store can carry none.
    pub tags: HashMap<String, String>,
    /// The resource-based policy attached to this trust store, which
    /// `GetResourcePolicy` reports. `None` for a trust store no policy was attached to:
    /// a policy is what SHARING a trust store with another account writes, and this
    /// model declares no operation that writes one — it only reads a policy and deletes
    /// a share's association — so the record carries what was attached and a read
    /// answers from it rather than from a policy made up per call.
    pub resource_policy: Option<String>,
}

impl TrustStoreRecord {
    pub fn to_shape(&self) -> TrustStore {
        TrustStore {
            name: Some(self.name.clone()),
            trust_store_arn: Some(self.arn.clone()),
            status: Some(self.status.clone()),
            number_of_ca_certificates: Some(self.number_of_ca_certificates),
            // "The number of revoked certificates in the trust store" — what the
            // record holds, never a figure computed here.
            total_revoked_entries: Some(self.total_revoked_entries),
        }
    }
}

/// A persisted listener RULE: one a caller added with `CreateRule`, which is every rule
/// of a listener except the default rule the listener itself carries
/// ([`ListenerRecord::default_rule`]).
///
/// A rule belongs to exactly one listener and is stored on that listener's record, so
/// releasing the listener releases its rules with it and no second store can disagree
/// about which listener a rule belongs to.
#[derive(Debug, Clone, Default)]
pub struct RuleRecord {
    pub arn: String,
    /// "Rules are evaluated in priority order, from the lowest value to the highest
    /// value" — which is the order a read walks them in. Modelled as an integer on the
    /// way in (`RulePriority`) and as text on the way out (`Rule$Priority`), so the
    /// record keeps the number and the shape renders it.
    pub priority: i32,
    pub conditions: Vec<RuleCondition>,
    pub actions: Vec<Action>,
    /// The transforms applied to a request this rule matches. Empty is reported as
    /// nothing rather than as an empty list, which is what a rule carrying none answers.
    pub transforms: Vec<RuleTransform>,
    /// The tags this rule was created with, which `DescribeTags` reports: a listener
    /// rule is a taggable Elastic Load Balancing resource.
    pub tags: HashMap<String, String>,
}

impl RuleRecord {
    pub fn to_shape(&self) -> Rule {
        Rule {
            rule_arn: Some(self.arn.clone()),
            priority: Some(self.priority.to_string()),
            // Each condition is reported under every spelling the model gives its
            // field, which is not always the spelling the create used
            // ([`reported_condition`]).
            conditions: Some(self.conditions.iter().map(reported_condition).collect()),
            actions: Some(self.actions.clone()),
            // Only the rule a listener mints with itself is the default one.
            is_default: Some(false),
            transforms: (!self.transforms.is_empty()).then(|| self.transforms.clone()),
        }
    }
}

/// One rule CONDITION as a read reports it: the arrays it matches on under every
/// spelling the model gives that field, and nothing under a spelling the field has none
/// of.
///
/// `RuleCondition` carries a condition's values twice over — at the top level (`Values`,
/// `RegexValues`) and inside the field's own configuration shape — and the model
/// documents the top-level member as the ALTERNATIVE to the configuration one for the
/// fields it names: `Values` is to be specified "only when Field is host-header or
/// path-pattern. Alternatively, to specify multiple host names or multiple path patterns,
/// use HostHeaderConfig or PathPatternConfig", and `RegexValues` "only when Field is
/// http-header, host-header, or path-pattern". Two spellings of ONE array, so a read
/// answers both of the spellings a field has whichever one the create carried, and
/// reports a member the field has no top-level spelling for not at all — absent, which is
/// a different answer from an empty list.
fn reported_condition(condition: &RuleCondition) -> RuleCondition {
    let mut reported = condition.clone();
    let field = condition.field.clone().unwrap_or_default();
    match field.as_str() {
        "host-header" => {
            let mut config = reported.host_header_config.take().unwrap_or_default();
            mirror_condition_values(&mut reported.values, &mut config.values);
            mirror_condition_values(&mut reported.regex_values, &mut config.regex_values);
            reported.host_header_config =
                (config.values.is_some() || config.regex_values.is_some()).then_some(config);
        }
        "path-pattern" => {
            let mut config = reported.path_pattern_config.take().unwrap_or_default();
            mirror_condition_values(&mut reported.values, &mut config.values);
            mirror_condition_values(&mut reported.regex_values, &mut config.regex_values);
            reported.path_pattern_config =
                (config.values.is_some() || config.regex_values.is_some()).then_some(config);
        }
        "http-header" => {
            // This field has a top-level spelling for its REGULAR EXPRESSIONS and none
            // for its values: the header a condition matches on and the strings it
            // compares against are named together, inside the configuration shape.
            reported.values = None;
            let mut config = reported.http_header_config.take().unwrap_or_default();
            mirror_condition_values(&mut reported.regex_values, &mut config.regex_values);
            reported.http_header_config = (config.http_header_name.is_some()
                || config.values.is_some()
                || config.regex_values.is_some())
            .then_some(config);
        }
        // Every other documented field — `query-string`, `http-request-method` and
        // `source-ip` — the model gives no top-level spelling at all, so neither member
        // is reported for it.
        _ => {
            reported.values = None;
            reported.regex_values = None;
        }
    }
    reported
}

/// Fill in whichever of the two spellings of one condition array was left out, leaving a
/// spelling the caller gave exactly as it was given.
///
/// An EMPTY array is not mirrored: the model states that "the value for a condition can't
/// be empty", so there is nothing in it to answer under the other spelling, and a member
/// the request carried empty stays empty rather than turning into an absent one.
fn mirror_condition_values(direct: &mut Option<Vec<String>>, configured: &mut Option<Vec<String>>) {
    match (direct.as_deref(), configured.as_deref()) {
        (Some(values), None) if !values.is_empty() => *configured = direct.clone(),
        (None, Some(values)) if !values.is_empty() => *direct = configured.clone(),
        _ => {}
    }
}

/// A persisted listener.
#[derive(Debug, Clone, Default)]
pub struct ListenerRecord {
    pub arn: String,
    pub load_balancer_arn: String,
    pub port: Option<i32>,
    pub protocol: Option<String>,
    pub ssl_policy: Option<String>,
    /// The listener's DEFAULT certificate — "[HTTPS and TLS listeners] The default
    /// certificate for the listener. You must provide exactly one certificate." — which
    /// is the one `CreateListener` and `ModifyListener` set and the only one a `Listener`
    /// shape reports.
    pub certificate_arns: Vec<String>,
    /// The listener's CERTIFICATE LIST: the additional certificates
    /// `AddListenerCertificates` puts on an HTTPS or TLS listener for SNI, in the order
    /// they were added. Separate from the default certificate above, because the two are
    /// managed by different operations and `DescribeListenerCertificates` reports them
    /// apart (`Certificate$IsDefault`).
    pub list_certificate_arns: Vec<String>,
    pub default_actions: Vec<Action>,
    pub alpn_policy: Vec<String>,
    pub attributes: HashMap<String, String>,
    pub tags: HashMap<String, String>,
    /// The resource id of this listener's DEFAULT rule, minted with the listener: every
    /// listener has one, it carries the listener's default actions, and it is what
    /// `DescribeRules` reports for a listener nothing else has added a rule to.
    pub default_rule_id: String,
    /// The rules a caller added to this listener with `CreateRule`, in the order they
    /// were added; a read orders them by priority
    /// ([`ComAmazonawsElasticloadbalancingv2ElasticloadbalancingV10State::rules_of`]).
    pub rules: Vec<RuleRecord>,
}

impl ListenerRecord {
    /// The settings `CreateListener` is idempotent over. Actions are compared by the
    /// members this slice models — their type and the target group they forward to —
    /// because the whole action tree is stored but not all of it is comparable.
    pub fn has_settings_of(&self, other: &ListenerRecord) -> bool {
        self.protocol == other.protocol
            && self.ssl_policy == other.ssl_policy
            && sorted(&self.certificate_arns) == sorted(&other.certificate_arns)
            && self.alpn_policy == other.alpn_policy
            && action_summary(&self.default_actions) == action_summary(&other.default_actions)
    }

    pub fn to_shape(&self) -> Listener {
        Listener {
            listener_arn: Some(self.arn.clone()),
            load_balancer_arn: Some(self.load_balancer_arn.clone()),
            port: self.port,
            protocol: self.protocol.clone(),
            certificates: Some(
                self.certificate_arns
                    .iter()
                    .map(|arn| Certificate {
                        certificate_arn: Some(arn.clone()),
                        // `Certificate$IsDefault`: "This value is not included in the
                        // output when describing a listener, but is included when
                        // describing listener certificates."
                        is_default: None,
                    })
                    .collect(),
            ),
            ssl_policy: self.ssl_policy.clone(),
            default_actions: Some(self.default_actions.clone()),
            alpn_policy: Some(self.alpn_policy.clone()),
            mutual_authentication: None,
        }
    }

    /// Whether this listener terminates TLS, and so carries certificates at all.
    pub fn terminates_tls(&self) -> bool {
        terminates_tls(self.protocol.as_deref())
    }

    /// What `DescribeListenerCertificates` reports: the default certificate, then the
    /// certificate list.
    ///
    /// The operation is documented as describing "the default certificate and the
    /// certificate list for the specified HTTPS or TLS listener", and `IsDefault`
    /// distinguishes them, so a certificate that is in both appears twice — once as the
    /// default and once as a list entry — rather than being collapsed.
    pub fn certificate_shapes(&self) -> Vec<Certificate> {
        self.certificate_arns
            .iter()
            .map(|arn| Certificate {
                certificate_arn: Some(arn.clone()),
                is_default: Some(true),
            })
            .chain(self.list_certificate_arns.iter().map(|arn| Certificate {
                certificate_arn: Some(arn.clone()),
                is_default: Some(false),
            }))
            .collect()
    }

    /// The ARN a rule of this listener carrying the resource id `rule_id` has.
    ///
    /// A listener rule's ARN is its listener's with the resource type `listener-rule` and
    /// the rule's own id appended — the segments this service's ARNs carry
    /// (`listener/<type>/<name>/<lb-id>/<id>` and
    /// `listener-rule/<type>/<name>/<lb-id>/<listener-id>/<id>`), so it is derived from
    /// the stored listener ARN rather than rebuilt from parts. Every rule of a listener,
    /// the default one included, is named through here, so one place decides the form.
    pub fn rule_arn(&self, rule_id: &str) -> String {
        format!(
            "{}/{rule_id}",
            self.arn.replacen(":listener/", ":listener-rule/", 1)
        )
    }

    /// The ARN of this listener's default rule.
    pub fn default_rule_arn(&self) -> String {
        self.rule_arn(&self.default_rule_id)
    }

    /// This listener's default rule: the rule that forwards everything the listener's
    /// own rules do not match, which is why its actions ARE the listener's default
    /// actions and it matches on no condition.
    pub fn default_rule(&self) -> Rule {
        Rule {
            rule_arn: Some(self.default_rule_arn()),
            // The one priority AWS reports for a default rule; every other rule's is a
            // number.
            priority: Some("default".to_string()),
            conditions: Some(Vec::new()),
            actions: Some(self.default_actions.clone()),
            is_default: Some(true),
            transforms: None,
        }
    }

    /// Every attribute a listener of a load balancer in `family` reports: the value
    /// stored for it, else the default its documentation states, and nothing for an
    /// attribute this family's listeners do not carry or whose default the model does
    /// not state.
    pub fn effective_attributes(&self, family: LoadBalancerFamily) -> Vec<(String, String)> {
        LISTENER_ATTRIBUTES
            .iter()
            .filter(|spec| spec.applies_to(family))
            .filter_map(|spec| {
                let value = self
                    .attributes
                    .get(spec.key)
                    .cloned()
                    .or_else(|| spec.default.map(str::to_string))?;
                Some((spec.key.to_string(), value))
            })
            .collect()
    }

    /// [`Self::effective_attributes`] as the modelled shapes a describe answers with.
    pub fn attribute_shapes(&self, family: LoadBalancerFamily) -> Vec<ListenerAttribute> {
        self.effective_attributes(family)
            .into_iter()
            .map(|(key, value)| ListenerAttribute {
                key: Some(key),
                value: Some(value),
            })
            .collect()
    }

    /// Every target group an ACTION of this listener forwards to: its default actions,
    /// and the actions of every rule a caller added to it.
    ///
    /// One place answers "what does this listener route to", because two questions are
    /// decided by it — which load balancers a target group reports under
    /// `LoadBalancerArns`, and whether a target group is "referenced by any actions" and
    /// so cannot be deleted.
    pub fn forwarded_target_group_arns(&self) -> Vec<String> {
        let mut arns = action_target_group_arns(&self.default_actions);
        for rule in &self.rules {
            arns.extend(action_target_group_arns(&rule.actions));
        }
        arns
    }
}

/// Every target group ARN an action forwards to, whether named directly or through a
/// `ForwardConfig`.
pub fn action_target_group_arns(actions: &[Action]) -> Vec<String> {
    let mut arns = Vec::new();
    for action in actions {
        if let Some(arn) = action.target_group_arn.as_ref() {
            arns.push(arn.clone());
        }
        if let Some(config) = action.forward_config.as_ref() {
            for tuple in config.target_groups.iter().flatten() {
                if let Some(arn) = tuple.target_group_arn.as_ref() {
                    arns.push(arn.clone());
                }
            }
        }
    }
    arns
}

/// The comparable projection of an action list: each action's type, order and the
/// target groups it forwards to.
fn action_summary(actions: &[Action]) -> Vec<(String, Option<i32>, Vec<String>)> {
    actions
        .iter()
        .map(|action| {
            (
                action.r#type.clone(),
                action.order,
                action_target_group_arns(std::slice::from_ref(action)),
            )
        })
        .collect()
}

/// The kind of Elastic Load Balancing resource an ARN names.
///
/// Every taggable resource of this service has its own `NotFound` error, so an ARN that
/// names nothing is refused with the one belonging to the kind the ARN itself declares
/// — which is the resource segment of the ARN, not anything the store holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggableResource {
    LoadBalancer,
    TargetGroup,
    Listener,
    Rule,
    TrustStore,
}

/// The kind of resource `arn` names, read from the resource TYPE of the ARN — the
/// segments AWS publishes for this service's ARNs (`loadbalancer/<type>/<name>/<id>`,
/// `targetgroup/<name>/<id>`, `listener/<type>/<name>/<lb-id>/<id>`,
/// `listener-rule/<type>/<name>/<lb-id>/<listener-id>/<id>`, `truststore/<name>/<id>`),
/// which are the forms the create handlers in `src/ops/` mint.
///
/// The ARN itself is read through `service_common::arn`, the workspace's ONE parse of a
/// caller-supplied resource ARN, narrowed to this service's own vendor: an ARN of
/// another vendor, or a value that is not a well-formed ARN, names nothing this API can
/// address, which is a malformed request rather than a missing resource — so this
/// answers `None` for it. The partition, Region and account are deliberately NOT
/// checked: an ARN of another Region is well formed and simply names no resource this
/// store holds.
pub fn taggable_resource(arn: &str) -> Option<TaggableResource> {
    match ::service_common::arn::parse_of(arn, SERVICE_NAMESPACE)?.resource_type() {
        "loadbalancer" => Some(TaggableResource::LoadBalancer),
        "targetgroup" => Some(TaggableResource::TargetGroup),
        "listener" => Some(TaggableResource::Listener),
        "listener-rule" => Some(TaggableResource::Rule),
        "truststore" => Some(TaggableResource::TrustStore),
        _ => None,
    }
}

/// The ARN segment AWS uses for each load balancer type: `app` for an Application
/// Load Balancer, `net` for a Network Load Balancer, `gwy` for a Gateway Load
/// Balancer. Both load balancer and listener ARNs carry it.
pub fn load_balancer_type_code(lb_type: &str) -> &'static str {
    match lb_type {
        "network" => "net",
        "gateway" => "gwy",
        // `application` is the modelled default for `Type`.
        _ => "app",
    }
}

fn sorted(values: &[String]) -> Vec<String> {
    let mut sorted = values.to_vec();
    sorted.sort();
    sorted
}

/// Per-service state for com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsElasticloadbalancingv2ElasticloadbalancingV10State {
    /// Load balancers by ARN.
    pub load_balancers: HashMap<String, LoadBalancerRecord>,
    /// Load balancer name to ARN, so a name is unique and `Names` lookups are direct.
    pub name_to_lb_arn: HashMap<String, String>,
    /// Target groups by ARN.
    pub target_groups: HashMap<String, TargetGroupRecord>,
    /// Target group name to ARN.
    pub name_to_tg_arn: HashMap<String, String>,
    /// Listeners by ARN.
    pub listeners: HashMap<String, ListenerRecord>,
    /// Trust stores by ARN.
    pub trust_stores: HashMap<String, TrustStoreRecord>,
    /// Trust store name to ARN: the model documents a trust store's name as unique
    /// per region, so `Names` lookups resolve through it directly.
    pub name_to_trust_store_arn: HashMap<String, String>,
    /// The counter the resource-id generator mixes, so an ARN is stable and unique
    /// within a run. A counter and a seed, not a store: nothing accumulates here.
    next_id: u64,
    id_seed: std::collections::hash_map::RandomState,
}

impl ComAmazonawsElasticloadbalancingv2ElasticloadbalancingV10State {
    /// A fresh 16-hex resource id, carried by no resource this store holds. AWS
    /// resource ids are opaque, so this mixes a per-store seed with a counter rather
    /// than exposing the counter itself.
    ///
    /// Uniqueness is checked against the LIVE resources — which the quotas above
    /// bound — rather than against a ledger of every id ever minted, so nothing here
    /// grows with the number of calls.
    pub fn mint_id(&mut self) -> String {
        use std::hash::{BuildHasher, Hasher};
        loop {
            self.next_id = self.next_id.wrapping_add(1);
            let mut hasher = self.id_seed.build_hasher();
            hasher.write_u64(self.next_id);
            let id = format!("{:016x}", hasher.finish());
            if !self.id_in_use(&id) {
                return id;
            }
        }
    }

    /// Whether any load balancer, target group, listener or rule in the store already
    /// carries `id`.
    fn id_in_use(&self, id: &str) -> bool {
        let suffix = format!("/{id}");
        self.load_balancers.values().any(|lb| lb.id == id)
            || self.target_groups.keys().any(|arn| arn.ends_with(&suffix))
            || self.listeners.keys().any(|arn| arn.ends_with(&suffix))
            || self.listeners.values().any(|listener| {
                listener.default_rule_id == id
                    || listener.rules.iter().any(|rule| rule.arn.ends_with(&suffix))
            })
            || self.trust_stores.keys().any(|arn| arn.ends_with(&suffix))
    }

    /// Materialize the state moves the clock already made due.
    ///
    /// A reporting operation calls this before it reads: a load balancer's move from
    /// `provisioning` to `active`, and a trust store's from `CREATING` to `ACTIVE`,
    /// were owed at [`SETTLE_AFTER`] past the create, even if nobody had called
    /// since.
    pub fn settle(&mut self, now: SystemTime) {
        for lb in self.load_balancers.values_mut() {
            if lb.state_code == "provisioning" && now >= lb.active_at {
                lb.state_code = "active".to_string();
            }
        }
        for store in self.trust_stores.values_mut() {
            if store.status == TRUST_STORE_CREATING && now >= store.active_at {
                store.status = TRUST_STORE_ACTIVE.to_string();
            }
        }
    }

    /// The tags stored on the resource `arn` names, or `None` when this store holds no
    /// resource with that ARN.
    ///
    /// Every taggable resource keeps its tags on its own record, so one lookup per store
    /// finds them whatever kind the ARN names. A listener rule's record lives on its
    /// listener, which [`Self::rule_record`] resolves through the ARN itself rather than
    /// through a scan.
    pub fn tags_of(&self, arn: &str) -> Option<&HashMap<String, String>> {
        self.load_balancers
            .get(arn)
            .map(|record| &record.tags)
            .or_else(|| self.target_groups.get(arn).map(|record| &record.tags))
            .or_else(|| self.listeners.get(arn).map(|record| &record.tags))
            .or_else(|| self.trust_stores.get(arn).map(|record| &record.tags))
            .or_else(|| self.rule_record(arn).map(|record| &record.tags))
    }

    /// Load balancers in a stable order (oldest first, ARN breaking ties), which is
    /// the order pagination walks.
    pub fn load_balancers_in_order(&self) -> Vec<&LoadBalancerRecord> {
        let mut records: Vec<&LoadBalancerRecord> = self.load_balancers.values().collect();
        records.sort_by(|a, b| {
            a.created_time
                .cmp(&b.created_time)
                .then_with(|| a.arn.cmp(&b.arn))
        });
        records
    }

    /// Target groups in a stable order (by ARN).
    pub fn target_groups_in_order(&self) -> Vec<&TargetGroupRecord> {
        let mut records: Vec<&TargetGroupRecord> = self.target_groups.values().collect();
        records.sort_by(|a, b| a.arn.cmp(&b.arn));
        records
    }

    /// Trust stores in a stable order (oldest first, ARN breaking ties), which is the
    /// order pagination walks.
    pub fn trust_stores_in_order(&self) -> Vec<&TrustStoreRecord> {
        let mut records: Vec<&TrustStoreRecord> = self.trust_stores.values().collect();
        records.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.arn.cmp(&b.arn))
        });
        records
    }

    /// Listeners in a stable order (by ARN).
    pub fn listeners_in_order(&self) -> Vec<&ListenerRecord> {
        let mut records: Vec<&ListenerRecord> = self.listeners.values().collect();
        records.sort_by(|a, b| a.arn.cmp(&b.arn));
        records
    }

    /// Record that `load_balancer_arn` forwards to `target_group_arn`.
    pub fn associate_target_group(&mut self, target_group_arn: &str, load_balancer_arn: &str) {
        if let Some(group) = self.target_groups.get_mut(target_group_arn) {
            if !group
                .load_balancer_arns
                .iter()
                .any(|arn| arn == load_balancer_arn)
            {
                group.load_balancer_arns.push(load_balancer_arn.to_string());
            }
        }
    }

    /// Remove a load balancer, its listeners and every association its listeners
    /// made, answering whether it was there.
    pub fn remove_load_balancer(&mut self, arn: &str) -> bool {
        let Some(removed) = self.load_balancers.remove(arn) else {
            return false;
        };
        self.name_to_lb_arn.remove(&removed.name);
        for listener_arn in &removed.listener_arns {
            self.listeners.remove(listener_arn);
        }
        for group in self.target_groups.values_mut() {
            group.load_balancer_arns.retain(|lb| lb != arn);
        }
        true
    }

    /// Remove ONE listener — from the store, from its load balancer's listener list and
    /// from every target group no remaining listener of that load balancer forwards to —
    /// answering whether it was there.
    pub fn remove_listener(&mut self, arn: &str) -> bool {
        let Some(removed) = self.listeners.remove(arn) else {
            return false;
        };
        let load_balancer_arn = removed.load_balancer_arn.clone();
        if let Some(load_balancer) = self.load_balancers.get_mut(&load_balancer_arn) {
            load_balancer.listener_arns.retain(|listener| listener != arn);
        }
        self.refresh_target_group_associations(&load_balancer_arn);
        true
    }

    /// Make every target group's `LoadBalancerArns` agree with the listeners this store
    /// holds for `load_balancer_arn`: a target group some listener of it forwards to
    /// lists it, and one no listener forwards to does not.
    ///
    /// Called after a listener's actions change or a listener goes away, so the
    /// association a target group reports is derived from the listeners that exist
    /// rather than accumulated. The association is dropped only when nothing else keeps
    /// it: a second listener of the same load balancer still forwarding there does, and
    /// so does a RULE of one — a load balancer routes to a target group an action of any
    /// of its rules forwards to, not only one its default actions name.
    pub fn refresh_target_group_associations(&mut self, load_balancer_arn: &str) {
        let forwarded: Vec<String> = self
            .listeners
            .values()
            .filter(|listener| listener.load_balancer_arn == load_balancer_arn)
            .flat_map(ListenerRecord::forwarded_target_group_arns)
            .collect();
        for (arn, group) in &mut self.target_groups {
            let holds = group
                .load_balancer_arns
                .iter()
                .any(|lb| lb == load_balancer_arn);
            if forwarded.contains(arn) {
                if !holds {
                    group.load_balancer_arns.push(load_balancer_arn.to_string());
                }
            } else if holds {
                group.load_balancer_arns.retain(|lb| lb != load_balancer_arn);
            }
        }
    }

    /// The load balancer family of the listener `arn` names, or `None` when the store
    /// holds no such listener. Which attributes a listener carries follows from it.
    pub fn listener_family(&self, arn: &str) -> Option<LoadBalancerFamily> {
        let listener = self.listeners.get(arn)?;
        let load_balancer = self.load_balancers.get(&listener.load_balancer_arn)?;
        Some(load_balancer_family(&load_balancer.lb_type))
    }

    /// Every rule of one listener, in the order a paginated read walks them.
    ///
    /// A listener's rules are the ones a caller added, in EVALUATION order — "rules are
    /// evaluated in priority order, from the lowest value to the highest value" — and
    /// then its DEFAULT rule, which is last because it is what runs when no other rule's
    /// conditions are met. Two rules of one listener never share a priority
    /// (`CreateRule` refuses that), so the order is total; the ARN breaks a tie only so
    /// that a page walk is stable whatever the store's iteration order.
    pub fn rules_of(&self, listener: &ListenerRecord) -> Vec<Rule> {
        let mut added: Vec<&RuleRecord> = listener.rules.iter().collect();
        added.sort_by(|left, right| {
            left.priority
                .cmp(&right.priority)
                .then_with(|| left.arn.cmp(&right.arn))
        });
        let mut rules: Vec<Rule> = added.into_iter().map(RuleRecord::to_shape).collect();
        rules.push(listener.default_rule());
        rules
    }

    /// The record of the rule `arn` names, or `None` when the listener it belongs to
    /// holds no such rule. The listener's DEFAULT rule has no record: it is derived from
    /// the listener itself ([`ListenerRecord::default_rule`]).
    ///
    /// ONE listener lookup, never a scan over the store: a rule ARN is its listener's
    /// with the resource type `listener-rule` and the rule's own id appended
    /// ([`ListenerRecord::default_rule_arn`]), so the owning listener is a pure string
    /// transform of the value asked for.
    pub fn rule_record(&self, arn: &str) -> Option<&RuleRecord> {
        let listener = self.listeners.get(&rule_listener_arn(arn)?)?;
        listener.rules.iter().find(|rule| rule.arn == arn)
    }

    /// The rule `arn` names, or `None` when no listener in this store holds it.
    ///
    /// Resolved through the same one listener lookup [`Self::rule_record`] uses, so the
    /// cost of resolving one rule does not grow with the number of listeners the store
    /// holds; the listener's own rule list is bounded by [`RULES_PER_LOAD_BALANCER`].
    pub fn rule_by_arn(&self, arn: &str) -> Option<Rule> {
        let listener = self.listeners.get(&rule_listener_arn(arn)?)?;
        self.rules_of(listener)
            .into_iter()
            .find(|rule| rule.rule_arn.as_deref() == Some(arn))
    }

    /// Remove ONE rule a caller added — from the listener that holds it, and with it
    /// every target group association no remaining action of its load balancer keeps
    /// alive — answering whether it was there.
    ///
    /// The owning listener is resolved from the rule ARN itself, as [`Self::rule_record`]
    /// resolves it, so nothing scans the store. A listener's DEFAULT rule is NOT removed
    /// here: it has no record of its own, being derived from the listener
    /// ([`ListenerRecord::default_rule`]), and the operation that would remove it refuses
    /// instead.
    pub fn remove_rule(&mut self, arn: &str) -> bool {
        let Some(listener_arn) = rule_listener_arn(arn) else {
            return false;
        };
        let Some(listener) = self.listeners.get_mut(&listener_arn) else {
            return false;
        };
        let held = listener.rules.len();
        listener.rules.retain(|rule| rule.arn != arn);
        if listener.rules.len() == held {
            return false;
        }
        // A load balancer routes to a target group an action of any of its rules forwards
        // to, so the rule going away can be what releases that association — and what
        // lets the target group be deleted.
        let load_balancer_arn = listener.load_balancer_arn.clone();
        self.refresh_target_group_associations(&load_balancer_arn);
        true
    }

    /// How many rules a caller has added across every listener of `load_balancer_arn`,
    /// which is what [`RULES_PER_LOAD_BALANCER`] bounds.
    pub fn rule_count_of_load_balancer(&self, load_balancer_arn: &str) -> usize {
        self.listeners
            .values()
            .filter(|listener| listener.load_balancer_arn == load_balancer_arn)
            .map(|listener| listener.rules.len())
            .sum()
    }
}

/// The ARN of the listener the rule `arn` belongs to, or `None` when `arn` carries no
/// rule id at all. A pure string transform of the rule ARN: the two differ in their
/// resource type and in the rule id the rule ARN appends.
fn rule_listener_arn(arn: &str) -> Option<String> {
    let (listener_part, _rule_id) = arn.rsplit_once('/')?;
    Some(listener_part.replacen(":listener-rule/", ":listener/", 1))
}

/// Why a page request could not be served.
#[derive(Debug)]
pub enum PageError {
    /// The `Marker` did not come from a previous page of this collection.
    InvalidMarker,
}

/// One page of `items`, plus the marker that resumes after it.
///
/// The marker is the index the next page starts at, which is why every paginated
/// read here walks its collection in a stable order. A marker that is not a
/// position in the collection — not a number, or past its end — is refused;
/// nothing in it binds it to the page that issued it.
pub fn paginate<T>(
    items: Vec<T>,
    marker: Option<&str>,
    page_size: Option<i32>,
) -> Result<(Vec<T>, Option<String>), PageError> {
    let start = match marker {
        None => 0usize,
        Some(marker) => marker.parse::<usize>().map_err(|_| PageError::InvalidMarker)?,
    };
    if start > items.len() {
        return Err(PageError::InvalidMarker);
    }
    let mut page: Vec<T> = items.into_iter().skip(start).collect();
    let next = match page_size.and_then(|size| usize::try_from(size).ok()) {
        Some(size) if page.len() > size => {
            page.truncate(size);
            Some((start + size).to_string())
        }
        _ => None,
    };
    Ok((page, next))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsElasticloadbalancingv2ElasticloadbalancingV10State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsElasticloadbalancingv2ElasticloadbalancingV10State>)
}
