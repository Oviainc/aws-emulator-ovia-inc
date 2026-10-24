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

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Action, AvailabilityZone, Certificate, Listener, LoadBalancer, LoadBalancerAddress,
    LoadBalancerState, Matcher, TargetDescription, TargetGroup, TargetGroupAttribute, TargetHealth,
    TargetHealthDescription, TrustStore,
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

/// How many targets one target group holds before `RegisterTargets` refuses with the
/// modelled `TooManyTargetsException`. The emulator's own bound, as
/// [`LOAD_BALANCER_QUOTA`]; registrations live on their target group, so they are
/// bounded by this times [`TARGET_GROUP_QUOTA`].
pub const TARGETS_PER_TARGET_GROUP: usize = 1_000;

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
    fn to_shape(&self) -> AvailabilityZone {
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
            enforce_security_group_inbound_rules_on_private_link_traffic: None,
            enable_prefix_for_ipv6_source_nat: None,
            ipam_pools: None,
        }
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
    fn to_shape(&self) -> Matcher {
        Matcher {
            http_code: self.http_code.clone(),
            grpc_code: self.grpc_code.clone(),
        }
    }
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
/// none: `200` for an HTTP target group, and the gRPC success code `12` for a gRPC one
/// (`Matcher`: "[HTTP/HTTPS health checks] The HTTP or gRPC codes to use when checking
/// for a successful response from a target").
pub fn default_health_check_matcher(protocol_version: Option<&str>) -> MatcherRecord {
    if protocol_version == Some("GRPC") {
        MatcherRecord {
            http_code: None,
            grpc_code: Some("12".to_string()),
        }
    } else {
        MatcherRecord {
            http_code: Some("200".to_string()),
            grpc_code: None,
        }
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

/// The published maximum length of a target group name, from `CreateTargetGroup`'s
/// `Name` documentation: "can have a maximum of 32 characters".
pub const TARGET_GROUP_NAME_MAX_LENGTH: usize = 32;

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

/// The set of values one target group attribute accepts, as its own documentation
/// states it (`smithy_query(query="doc",shape="TargetGroupAttribute",member="Key")`).
#[derive(Debug, Clone, Copy)]
pub enum AttributeDomain {
    /// "The value is `true` or `false`."
    Boolean,
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
        match self.protocol.as_deref() {
            Some("GENEVE") => LoadBalancerFamily::Gateway,
            Some("TCP") | Some("TLS") | Some("UDP") | Some("TCP_UDP") | Some("QUIC")
            | Some("TCP_QUIC") => LoadBalancerFamily::Network,
            // HTTP, HTTPS, and a `lambda` target group, which carries no protocol.
            _ => LoadBalancerFamily::Application,
        }
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
                self.health_check.matcher =
                    Some(default_health_check_matcher(protocol_version.as_deref()));
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
    /// port, or — for the modelled default `traffic-port` — the port the target
    /// receives traffic on.
    pub fn health_check_port_of(&self, target: &RegisteredTarget) -> Option<String> {
        match self.health_check.port.as_deref() {
            Some("traffic-port") | None => target.port.or(self.port).map(|p| p.to_string()),
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

/// A persisted listener.
#[derive(Debug, Clone, Default)]
pub struct ListenerRecord {
    pub arn: String,
    pub load_balancer_arn: String,
    pub port: Option<i32>,
    pub protocol: Option<String>,
    pub ssl_policy: Option<String>,
    pub certificate_arns: Vec<String>,
    pub default_actions: Vec<Action>,
    pub alpn_policy: Vec<String>,
    pub attributes: HashMap<String, String>,
    pub tags: HashMap<String, String>,
    pub rule_arns: Vec<String>,
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
                        is_default: Some(true),
                    })
                    .collect(),
            ),
            ssl_policy: self.ssl_policy.clone(),
            default_actions: Some(self.default_actions.clone()),
            alpn_policy: Some(self.alpn_policy.clone()),
            mutual_authentication: None,
        }
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

    /// Whether any load balancer, target group or listener in the store already
    /// carries `id`.
    fn id_in_use(&self, id: &str) -> bool {
        let suffix = format!("/{id}");
        self.load_balancers.values().any(|lb| lb.id == id)
            || self.target_groups.keys().any(|arn| arn.ends_with(&suffix))
            || self.listeners.keys().any(|arn| arn.ends_with(&suffix))
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
    /// finds them whatever kind the ARN names. A listener rule is never found: this
    /// service's `CreateRule` is still a scaffold stub, so the store holds no rule for
    /// any ARN — the day it does, its record's tags join the chain below.
    pub fn tags_of(&self, arn: &str) -> Option<&HashMap<String, String>> {
        self.load_balancers
            .get(arn)
            .map(|record| &record.tags)
            .or_else(|| self.target_groups.get(arn).map(|record| &record.tags))
            .or_else(|| self.listeners.get(arn).map(|record| &record.tags))
            .or_else(|| self.trust_stores.get(arn).map(|record| &record.tags))
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
