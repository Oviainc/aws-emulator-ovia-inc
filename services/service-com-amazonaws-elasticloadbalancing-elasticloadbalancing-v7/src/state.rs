// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7 service.
//!
//! The primary resource is the Classic load balancer (the model's `AccessPoint`):
//! [`LoadBalancer`], keyed by `LoadBalancerName` in a [`BTreeMap`] so
//! `DescribeLoadBalancers` has one stable order to paginate over.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

/// The scheme a load balancer gets when the request names none.
///
/// From `CreateLoadBalancer`'s own documentation: "By default, Elastic Load Balancing
/// creates an Internet-facing load balancer".
pub const SCHEME_INTERNET_FACING: &str = "internet-facing";

/// The only other scheme `CreateLoadBalancer` documents: "Specify internal to create a
/// load balancer with a DNS name that resolves to private IP addresses."
pub const SCHEME_INTERNAL: &str = "internal";

/// The region an emulated request is attributed to when it carries no SigV4 scope.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The maximum number of load balancers one account holds in one region.
///
/// From `CreateLoadBalancer`'s own documentation: "You can create up to 20 load
/// balancers per region per account."
pub const MAX_LOAD_BALANCERS_PER_REGION: usize = 20;

/// The largest number of DISTINCT load balancers one `DescribeLoadBalancers` request may
/// name.
///
/// AWS publishes no quota for the member and accepts a list that repeats a name, so the
/// handler deduplicates first and applies this to the distinct names only, after each one
/// is known to exist. A region holds at most [`MAX_LOAD_BALANCERS_PER_REGION`] load
/// balancers, so a request whose names all exist can never reach this bound: it is the
/// backstop that keeps the work one request can ask for bounded by the store rather than
/// by the caller.
pub const MAX_LOAD_BALANCER_NAMES_PER_REQUEST: usize = MAX_LOAD_BALANCERS_PER_REGION;

/// The listener protocols the model documents for `Listener$Protocol` /
/// `Listener$InstanceProtocol`: "The load balancer transport protocol to use for
/// routing: HTTP, HTTPS, TCP, or SSL."
pub const LISTENER_PROTOCOLS: &[&str] = &["HTTP", "HTTPS", "TCP", "SSL"];

/// Published Classic Load Balancer quotas, from the "Elastic Load Balancing endpoints
/// and quotas" page of the AWS General Reference. Each one bounds a collection a
/// CALLER drives; together with [`MAX_REGIONS`], which bounds the one store dimension
/// AWS publishes no quota for, the store a request can grow is bounded too.
///
/// "Listeners per load balancer: 100".
pub const MAX_LISTENERS_PER_LOAD_BALANCER: usize = 100;

/// "Registered instances per load balancer: 1,000".
pub const MAX_INSTANCES_PER_LOAD_BALANCER: usize = 1_000;

/// "Tags per load balancer: 10".
pub const MAX_TAGS_PER_LOAD_BALANCER: usize = 10;

/// "Security groups per load balancer: 5".
pub const MAX_SECURITY_GROUPS_PER_LOAD_BALANCER: usize = 5;

/// The largest `AvailabilityZones` or `Subnets` list a create accepts.
///
/// AWS states this quota structurally ("one subnet per Availability Zone") rather than
/// as a number, and this emulator models no region topology, so this is a bound the
/// emulator chooses: comfortably above the Availability Zone count of any AWS region,
/// and low enough that one request cannot grow the store without limit.
pub const MAX_PLACEMENTS_PER_LOAD_BALANCER: usize = 50;

/// The largest `Instances` list one register/deregister/describe-health request may
/// carry: the same bound as the set it can grow, so the per-request work is bounded by
/// a constant rather than by the caller.
pub const MAX_INSTANCES_PER_REQUEST: usize = MAX_INSTANCES_PER_LOAD_BALANCER;

/// The number of distinct regions this store will hold records for.
///
/// Every published quota above is per region or per load balancer, so the region
/// dimension is the one a caller could otherwise multiply freely: the region comes from
/// the request's SigV4 credential scope, which the caller writes. AWS publishes no
/// quota here (a region is not a resource an account creates), so this is a bound the
/// emulator chooses, comfortably above the number of regions AWS publishes across all
/// partitions. A create that would open one region past it is refused.
pub const MAX_REGIONS: usize = 64;

/// The registered-instance set of one load balancer: deduplicated, in registration
/// order, with membership answered from an index rather than by scanning.
///
/// The ordered `Vec` is what `DescribeLoadBalancers` and `DescribeInstanceHealth`
/// report; the `BTreeSet` makes each membership test O(log n), so a batched request
/// costs O(batch x log n) instead of O(batch x n).
#[derive(Debug, Clone, Default)]
pub struct InstanceSet {
    order: Vec<String>,
    index: BTreeSet<String>,
}

impl InstanceSet {
    /// Add `instance_id` unless it is already registered. Answers whether it was added.
    pub fn insert(&mut self, instance_id: &str) -> bool {
        if !self.index.insert(instance_id.to_string()) {
            return false;
        }
        self.order.push(instance_id.to_string());
        true
    }

    /// Remove every id in `ids` in ONE pass over the ordered set, so a batch costs
    /// O(n log |ids|) rather than O(|ids| x n). Answers how many were registered.
    pub fn remove_all(&mut self, ids: &BTreeSet<String>) -> usize {
        let before = self.order.len();
        self.order.retain(|held| !ids.contains(held));
        for id in ids {
            self.index.remove(id);
        }
        before - self.order.len()
    }

    pub fn contains(&self, instance_id: &str) -> bool {
        self.index.contains(instance_id)
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// The registered ids in registration order.
    pub fn iter(&self) -> impl Iterator<Item = &String> {
        self.order.iter()
    }
}

/// One listener of a load balancer, keyed logically by its `LoadBalancerPort`.
#[derive(Debug, Clone)]
pub struct ListenerEntry {
    pub protocol: String,
    pub load_balancer_port: i32,
    /// Defaults to `protocol` when the create request omitted it.
    pub instance_protocol: String,
    pub instance_port: i32,
    pub ssl_certificate_id: Option<String>,
    pub policy_names: Vec<String>,
}

impl ListenerEntry {
    /// Whether `other` asks for the SAME listener this one already configures.
    ///
    /// The comparison is the one `DuplicateListenerException` is stated over — "A
    /// listener already exists for the specified load balancer name and port, but with a
    /// different instance port, protocol, or SSL certificate" — so a request that repeats
    /// a configured listener exactly is a no-op rather than a conflict, and one that
    /// differs in any of those is the conflict that error names. `policy_names` is not
    /// part of it: a listener's policies are set by other operations, not by the create.
    pub fn configures_the_same(&self, other: &Self) -> bool {
        self.protocol == other.protocol
            && self.load_balancer_port == other.load_balancer_port
            && self.instance_protocol == other.instance_protocol
            && self.instance_port == other.instance_port
            && self.ssl_certificate_id == other.ssl_certificate_id
    }
}

/// Why a requested listener cannot be stored. Each variant is one rule the model states;
/// the OPERATION maps it onto the modelled error it declares for that rule, because the
/// two operations that create listeners declare different error sets.
#[derive(Debug, Clone)]
pub enum ListenerRejection {
    /// A `Protocol`/`InstanceProtocol` outside [`LISTENER_PROTOCOLS`].
    UnsupportedProtocol { protocol: String },
    /// A back-end protocol from the other transport family than the front-end one.
    IncompatibleProtocols {
        protocol: String,
        instance_protocol: String,
    },
}

/// Project one requested listener onto the record this service stores, applying the two
/// rules the model states over a listener's protocols.
///
/// `instance_protocol` of `None` takes the documented default ("The protocol to use for
/// routing traffic to instances" defaults to the front-end protocol), and both protocols
/// are stored upper-cased, the spelling the model's own documentation and
/// `DescribeLoadBalancers` use.
pub fn listener_entry(
    protocol: &str,
    load_balancer_port: i32,
    instance_protocol: Option<&str>,
    instance_port: i32,
    ssl_certificate_id: Option<&str>,
) -> Result<ListenerEntry, ListenerRejection> {
    let protocol = protocol.to_ascii_uppercase();
    let instance_protocol =
        instance_protocol.map_or_else(|| protocol.clone(), str::to_ascii_uppercase);

    for candidate in [&protocol, &instance_protocol] {
        if !LISTENER_PROTOCOLS.contains(&candidate.as_str()) {
            return Err(ListenerRejection::UnsupportedProtocol {
                protocol: candidate.clone(),
            });
        }
    }
    // From `Listener$InstanceProtocol`: "If the front-end protocol is TCP or SSL, the
    // back-end protocol must be TCP or SSL. If the front-end protocol is HTTP or HTTPS,
    // the back-end protocol must be HTTP or HTTPS."
    if transport_family(&protocol) != transport_family(&instance_protocol) {
        return Err(ListenerRejection::IncompatibleProtocols {
            protocol,
            instance_protocol,
        });
    }

    Ok(ListenerEntry {
        protocol,
        load_balancer_port,
        instance_protocol,
        instance_port,
        ssl_certificate_id: ssl_certificate_id.map(str::to_string),
        policy_names: Vec::new(),
    })
}

/// `HTTP`/`HTTPS` and `TCP`/`SSL` are the two families the model's compatibility rule is
/// stated over; a secure protocol belongs to the same family as its plain peer.
fn transport_family(protocol: &str) -> &'static str {
    match protocol {
        "HTTP" | "HTTPS" => "http",
        _ => "tcp",
    }
}

/// The maximum number of policies one load balancer holds.
///
/// AWS publishes no quota for policies per load balancer, and every policy is minted by
/// a CALLER, so this is a bound the emulator chooses to keep the store one caller can
/// grow finite. `TooManyPoliciesException` — "The quota for the number of policies for
/// this load balancer has been reached" — is the error every policy-creating operation
/// models for reaching it.
pub const MAX_POLICIES_PER_LOAD_BALANCER: usize = 100;

/// The `CookieExpirationPeriod` a duration-based stickiness policy gets when the create
/// request names none, from `CreateLBCookieStickinessPolicy`'s own documentation: "If you
/// do not specify this parameter, the default value is 0, which indicates that the sticky
/// session should last for the duration of the browser session."
pub const DEFAULT_COOKIE_EXPIRATION_PERIOD_SECONDS: i64 = 0;

/// What one stored policy configures: one variant per policy-creating operation, so the
/// list a describe projects a policy into follows from the stored record rather than from
/// a type string a handler wrote.
#[derive(Debug, Clone)]
pub enum PolicyKind {
    /// Created by `CreateLBCookieStickinessPolicy`: duration-based session stickiness,
    /// whose cookie goes stale after `cookie_expiration_period` seconds — 0 meaning "the
    /// duration of the browser session", the documented default.
    LbCookieStickiness { cookie_expiration_period: i64 },
}

/// One policy of a load balancer, as a policy-creating operation persists it and
/// `DescribeLoadBalancers` reports it.
///
/// A policy name "must be unique within the set of policies for this load balancer"
/// (`PolicyName`'s own documentation), so EVERY policy of one load balancer lives in the
/// one [`LoadBalancer::policies`] map whatever created it: a second map beside it would
/// let two creates mint two policies under one name.
#[derive(Debug, Clone)]
pub struct PolicyRecord {
    pub name: String,
    pub kind: PolicyKind,
}

/// Why `name` is not a policy name, or `Ok(())` when it is one.
///
/// From `PolicyName`'s documentation on every operation that creates a policy: "Policy
/// names must consist of alphanumeric characters and dashes (-)." The model declares the
/// member a plain string with no `@pattern` or `@length`, so nothing has refused a
/// malformed name by the time a handler runs. The message names the member and the rule
/// it broke, which is what a caller fixes the request from.
pub fn policy_name_rejection(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("PolicyName must not be empty".to_string());
    }
    match name
        .chars()
        .find(|character| !(character.is_ascii_alphanumeric() || *character == '-'))
    {
        Some(rejected) => Err(format!(
            "PolicyName must consist of alphanumeric characters and dashes (-); \
             '{name}' contains '{rejected}'"
        )),
        None => Ok(()),
    }
}

/// A load balancer's health-check configuration.
///
/// Seeded at create with the Classic Load Balancer defaults so
/// `DescribeLoadBalancers` reports a real `HealthCheck` before anyone calls
/// `ConfigureHealthCheck`.
#[derive(Debug, Clone)]
pub struct HealthCheckConfig {
    pub target: String,
    pub interval: i32,
    pub timeout: i32,
    pub unhealthy_threshold: i32,
    pub healthy_threshold: i32,
}

impl HealthCheckConfig {
    /// The default health check of a newly created load balancer: a TCP check against
    /// the instance port of its first listener.
    pub fn default_for_instance_port(instance_port: i32) -> Self {
        Self {
            target: format!("TCP:{instance_port}"),
            interval: 30,
            timeout: 5,
            unhealthy_threshold: 2,
            healthy_threshold: 10,
        }
    }
}

/// The idle connection timeout, in seconds, of a load balancer nobody has modified.
///
/// From `LoadBalancerAttributes$ConnectionSettings`' own documentation: "By default,
/// Elastic Load Balancing maintains a 60-second idle connection timeout for both
/// front-end and back-end connections of your load balancer."
pub const DEFAULT_IDLE_TIMEOUT_SECONDS: i32 = 60;

/// The `ConnectionSettings$IdleTimeout` bounds, which the model declares as
/// `@range(min: 1, max: 3600)`.
pub const IDLE_TIMEOUT_SECONDS: (i32, i32) = (1, 3600);

/// Whether connection draining is enabled on a load balancer nobody has modified, from
/// `ConnectionDraining$Enabled`'s modelled `@default(false)`.
pub const DEFAULT_CONNECTION_DRAINING_ENABLED: bool = false;

/// The connection-draining timeout, in seconds, of a load balancer nobody has modified.
///
/// "The default timeout is 300 seconds" — the "Configure Connection Draining for Your
/// Classic Load Balancer" page of the Classic Load Balancers Guide, which
/// `ConnectionDraining`'s documentation links to. The value is AWS's, not one this
/// emulator chose. Whether draining is ON by default is a separate question, answered by
/// [`DEFAULT_CONNECTION_DRAINING_ENABLED`] from the model's own `@default(false)`.
pub const DEFAULT_CONNECTION_DRAINING_TIMEOUT_SECONDS: i32 = 300;

/// The `ConnectionDraining$Timeout` bounds. The model declares no range for the member,
/// so these are the bounds the same guide publishes ("a value between 1 and 3,600
/// seconds").
pub const CONNECTION_DRAINING_TIMEOUT_SECONDS: (i32, i32) = (1, 3600);

/// The access-log publishing interval, in minutes, of a load balancer nobody has
/// modified. From `AccessLog$EmitInterval`: "Default: 60 minutes".
pub const DEFAULT_ACCESS_LOG_EMIT_INTERVAL_MINUTES: i32 = 60;

/// The only two intervals `AccessLog$EmitInterval` accepts: "You can specify an interval
/// of either 5 minutes or 60 minutes."
pub const ACCESS_LOG_EMIT_INTERVALS_MINUTES: &[i32] = &[5, 60];

/// The one additional attribute this service documents, from
/// `AdditionalAttribute$Key`: "`elb.http.desyncmitigationmode` - Determines how the load
/// balancer handles requests that might pose a security risk to your application."
pub const DESYNC_MITIGATION_MODE_KEY: &str = "elb.http.desyncmitigationmode";

/// Its accepted values, from the same sentence: "The possible values are `monitor`,
/// `defensive`, and `strictest`."
pub const DESYNC_MITIGATION_MODES: &[&str] = &["monitor", "defensive", "strictest"];

/// Its value on a load balancer nobody has modified: "The default is `defensive`."
pub const DEFAULT_DESYNC_MITIGATION_MODE: &str = "defensive";

/// A load balancer's `AccessLog` attribute.
#[derive(Debug, Clone)]
pub struct AccessLogConfig {
    pub enabled: bool,
    pub emit_interval_minutes: i32,
    pub s3_bucket_name: Option<String>,
    pub s3_bucket_prefix: Option<String>,
}

/// A load balancer's `ConnectionDraining` attribute.
#[derive(Debug, Clone)]
pub struct ConnectionDrainingConfig {
    pub enabled: bool,
    pub timeout_seconds: i32,
}

/// The attributes of one load balancer: every member
/// `DescribeLoadBalancerAttributes` reports and `ModifyLoadBalancerAttributes` writes.
///
/// Seeded at create with the service defaults, so a freshly created load balancer
/// reports the same attribute set AWS reports for one — rather than an empty document —
/// and a modification of one attribute leaves the others at whatever they held.
#[derive(Debug, Clone)]
pub struct AttributeConfig {
    pub cross_zone_load_balancing_enabled: bool,
    pub access_log: AccessLogConfig,
    pub connection_draining: ConnectionDrainingConfig,
    pub idle_timeout_seconds: i32,
    /// The `AdditionalAttributes` entries, keyed so one key holds one value and the list
    /// is reported in a stable order. Bounded by the member's modelled `@length(max: 10)`.
    pub additional: BTreeMap<String, String>,
}

impl Default for AttributeConfig {
    /// The attributes of a load balancer nobody has modified.
    fn default() -> Self {
        Self {
            // `CrossZoneLoadBalancing$Enabled` is modelled `@default(false)`.
            cross_zone_load_balancing_enabled: false,
            access_log: AccessLogConfig {
                // `AccessLog$Enabled` is modelled `@default(false)`.
                enabled: false,
                emit_interval_minutes: DEFAULT_ACCESS_LOG_EMIT_INTERVAL_MINUTES,
                s3_bucket_name: None,
                s3_bucket_prefix: None,
            },
            connection_draining: ConnectionDrainingConfig {
                enabled: DEFAULT_CONNECTION_DRAINING_ENABLED,
                timeout_seconds: DEFAULT_CONNECTION_DRAINING_TIMEOUT_SECONDS,
            },
            idle_timeout_seconds: DEFAULT_IDLE_TIMEOUT_SECONDS,
            additional: BTreeMap::from([(
                DESYNC_MITIGATION_MODE_KEY.to_string(),
                DEFAULT_DESYNC_MITIGATION_MODE.to_string(),
            )]),
        }
    }
}

/// One Classic load balancer, as persisted by `CreateLoadBalancer` and read back by
/// every other operation.
#[derive(Debug, Clone)]
pub struct LoadBalancer {
    pub name: String,
    /// Generated once at create and kept for the record's lifetime.
    pub dns_name: String,
    pub canonical_hosted_zone_name: String,
    pub canonical_hosted_zone_name_id: String,
    pub scheme: String,
    pub created_time: SystemTime,
    pub listeners: Vec<ListenerEntry>,
    pub availability_zones: Vec<String>,
    pub subnets: Vec<String>,
    pub security_groups: Vec<String>,
    pub vpc_id: Option<String>,
    /// Registered `InstanceId`s, deduplicated, in registration order, bounded by
    /// [`MAX_INSTANCES_PER_LOAD_BALANCER`].
    pub instances: InstanceSet,
    pub health_check: HealthCheckConfig,
    /// Every policy created for this load balancer, keyed by `PolicyName` so one name
    /// holds one policy and a describe reports them in one stable order. Bounded by
    /// [`MAX_POLICIES_PER_LOAD_BALANCER`].
    pub policies: BTreeMap<String, PolicyRecord>,
    /// The modifiable attributes `DescribeLoadBalancerAttributes` reports and
    /// `ModifyLoadBalancerAttributes` writes, seeded with the service defaults.
    pub attributes: AttributeConfig,
    pub tags: BTreeMap<String, String>,
}

impl LoadBalancer {
    /// Register `instance_id`, keeping the set deduplicated and in registration order.
    /// Answers whether the id was newly added.
    pub fn register_instance(&mut self, instance_id: &str) -> bool {
        self.instances.insert(instance_id)
    }

    /// Deregister every id in `ids`. Answers how many had been registered.
    pub fn deregister_instances(&mut self, ids: &BTreeSet<String>) -> usize {
        self.instances.remove_all(ids)
    }

    /// Whether `instance_id` is currently registered with this load balancer.
    pub fn has_instance(&self, instance_id: &str) -> bool {
        self.instances.contains(instance_id)
    }

    /// How many ids of `requested` are not registered yet — what registering them all
    /// would add to the stored set.
    pub fn new_instance_count<'a>(
        &self,
        requested: impl IntoIterator<Item = &'a String>,
    ) -> usize {
        let mut unseen: BTreeSet<&str> = BTreeSet::new();
        for id in requested {
            if !self.instances.contains(id) {
                unseen.insert(id.as_str());
            }
        }
        unseen.len()
    }
}

/// Per-service state for com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7. A handler reaches it through [`service_state`].
///
/// A load balancer is a REGIONAL resource: one `LoadBalancerName` is unique within one
/// region (the same name may name a different load balancer in another), a describe
/// signed for one region does not see another's, and the documented "20 load balancers
/// per region per account" quota is counted per region. The store is therefore keyed by
/// region first, and each region's own map is keyed by name.
#[derive(Debug, Default)]
pub struct ComAmazonawsElasticloadbalancingElasticloadbalancingV7State {
    /// Region -> (`LoadBalancerName` -> record). `BTreeMap`s throughout, so
    /// `DescribeLoadBalancers` paginates one stable order.
    pub regions: BTreeMap<String, BTreeMap<String, LoadBalancer>>,
}

impl ComAmazonawsElasticloadbalancingElasticloadbalancingV7State {
    /// The load balancers of `region`, or `None` when it holds none.
    pub fn load_balancers_in(&self, region: &str) -> Option<&BTreeMap<String, LoadBalancer>> {
        self.regions.get(region)
    }

    /// The load balancers of `region` for mutation, or `None` when it holds none. Does
    /// NOT create the region's map, so a read or a no-op delete leaves the store as it
    /// found it.
    pub fn load_balancers_in_mut(
        &mut self,
        region: &str,
    ) -> Option<&mut BTreeMap<String, LoadBalancer>> {
        self.regions.get_mut(region)
    }

    /// The load balancers of `region` for insertion, creating the region's (empty) map
    /// if this is the first record stored there. For a create path only, AFTER the
    /// request is known to be acceptable.
    ///
    /// `None` when the store already holds [`MAX_REGIONS`] regions and `region` is not
    /// one of them: the region comes from the caller's own credential scope, so the
    /// number of per-region budgets it can open is capped.
    pub fn region_entry(
        &mut self,
        region: &str,
    ) -> Option<&mut BTreeMap<String, LoadBalancer>> {
        if !self.regions.contains_key(region) && self.regions.len() >= MAX_REGIONS {
            return None;
        }
        Some(self.regions.entry(region.to_string()).or_default())
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsElasticloadbalancingElasticloadbalancingV7State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsElasticloadbalancingElasticloadbalancingV7State>)
}

/// The region this request is attributed to: the region field of its SigV4 credential
/// scope when it names a region, else [`DEFAULT_REGION`].
///
/// The scope is caller-written and the vendored parser accepts any non-empty field
/// there, so a value that is not shaped like an AWS region name is not treated as one:
/// it is attributed to the default region rather than opening a store partition of its
/// own.
pub fn region_from_request(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| looks_like_region(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `region` is shaped like a published AWS region name: three or four
/// hyphen-separated segments, the last a one- or two-digit number and the rest
/// lower-case letters — `us-east-1`, `ap-southeast-2`, `us-gov-west-1`, `cn-north-1`.
fn looks_like_region(region: &str) -> bool {
    let segments: Vec<&str> = region.split('-').collect();
    if !(3..=4).contains(&segments.len()) {
        return false;
    }
    let Some((number, words)) = segments.split_last() else {
        return false;
    };
    let words_look_right = words.iter().all(|word| {
        (1..=10).contains(&word.len()) && word.bytes().all(|byte| byte.is_ascii_lowercase())
    });
    let number_looks_right =
        (1..=2).contains(&number.len()) && number.bytes().all(|byte| byte.is_ascii_digit());
    words_look_right && number_looks_right
}

/// The first 16 bytes of SHA-256 over `label` and `region`, as one integer: a stable
/// per-region value to derive a generated id from.
fn region_digest(region: &str, label: &str) -> u128 {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(label.as_bytes());
    hasher.update(b"/");
    hasher.update(region.as_bytes());
    let digest = hasher.finalize();
    let mut head = [0u8; 16];
    head.copy_from_slice(&digest[..16]);
    u128::from_be_bytes(head)
}

/// A stable 10-digit id for a load balancer's DNS name, derived from the region and
/// name so the same record in the same region always resolves to the same host.
fn dns_suffix(region: &str, name: &str) -> u64 {
    // A 10-digit value, the shape Elastic Load Balancing's own DNS names carry.
    let value = (region_digest(region, name) % 9_000_000_000) as u64;
    1_000_000_000 + value
}

/// The DNS name of a load balancer: `<name>-<id>.<region>.elb.amazonaws.com`, prefixed
/// `internal-` for the `internal` scheme (whose name resolves to private addresses).
pub fn dns_name(region: &str, name: &str, scheme: &str) -> String {
    let suffix = dns_suffix(region, name);
    if scheme == SCHEME_INTERNAL {
        format!("internal-{name}-{suffix}.{region}.elb.amazonaws.com")
    } else {
        format!("{name}-{suffix}.{region}.elb.amazonaws.com")
    }
}

/// The published Route 53 hosted zone id of the Elastic Load Balancing (Classic)
/// endpoint in each region.
///
/// Source: the "Elastic Load Balancing endpoints and quotas" page of the AWS General
/// Reference, which publishes one hosted zone id per region for Classic Load Balancers.
const CLASSIC_ELB_HOSTED_ZONE_IDS: &[(&str, &str)] = &[
    ("af-south-1", "Z268VQBMOI5EKX"),
    ("ap-east-1", "Z3DQVH9N71FHZ0"),
    ("ap-northeast-1", "Z14GRHDCWA56QT"),
    ("ap-northeast-2", "ZWKZPGTI48KDX"),
    ("ap-northeast-3", "Z5LXEXXYQ4HTP"),
    ("ap-south-1", "ZP97RAFLXTNZK"),
    ("ap-southeast-1", "Z1LMS91P8CMLE5"),
    ("ap-southeast-2", "Z1GM3OXH4ZPM65"),
    ("ca-central-1", "ZQSVJUPU6J1EY"),
    ("eu-central-1", "Z215JYRZR1TBD5"),
    ("eu-north-1", "Z23TAZ6LKFMNIO"),
    ("eu-south-1", "Z3ULH7SSC9OV64"),
    ("eu-west-1", "Z32O12XQLNTSW2"),
    ("eu-west-2", "ZHURV8PSTC4K8"),
    ("eu-west-3", "Z3Q77PNBQS71R4"),
    ("me-south-1", "ZS929ML54UICD"),
    ("sa-east-1", "Z2P70J7HTTTPLU"),
    ("us-east-1", "Z35SXDOTRQ7X7K"),
    ("us-east-2", "Z3AADJGX6KTTL2"),
    ("us-west-1", "Z368ELLRRE2KJ0"),
    ("us-west-2", "Z1H1FL5HABSF5"),
];

/// The hosted zone id to report for a load balancer in `region`.
///
/// A region the published table above does not cover still gets a well-formed,
/// region-stable id (`Z` + 13 upper-case alphanumerics), because every load balancer
/// has exactly one hosted zone id and the field is not nullable in practice.
pub fn hosted_zone_id(region: &str) -> String {
    if let Some((_, id)) = CLASSIC_ELB_HOSTED_ZONE_IDS
        .iter()
        .find(|(candidate, _)| *candidate == region)
    {
        return (*id).to_string();
    }

    // 13 base-36 digits taken from one 128-bit digest of the region (36^13 < 2^128), so
    // an uncovered region always reports the same well-formed id.
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut value = region_digest(region, "hosted-zone");
    let mut id = String::from("Z");
    for _ in 0..13 {
        id.push(char::from(ALPHABET[(value % 36) as usize]));
        value /= 36;
    }
    id
}
