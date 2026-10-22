// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-elasticache-amazonelasticachev9 service.
//!
//! The cache cluster is ElastiCache's primary resource: `CreateCacheCluster`
//! mints one, `DescribeCacheClusters` reports it, `ModifyCacheCluster` changes
//! it and `DeleteCacheCluster` removes it. A cache subnet group is the
//! supporting resource a VPC cluster is placed in. The records below are the one
//! copy of each that every handler reads and writes, plus the projections into
//! the modelled `CacheCluster` / `CacheSubnetGroup` shapes, so the six handlers
//! agree on one implementation instead of carrying copies that can drift.
//!
//! Beside those customer-created resources sits the one catalog this service
//! publishes rather than mints: the service updates AWS releases for its cache
//! engines. No operation creates one (the API has no such call), so the store
//! starts out holding the catalog and `DescribeServiceUpdates` reports it.
//!
//! Everything else in this service (replication groups, serverless caches,
//! snapshots, users, parameter groups, migrations, update actions) is still a
//! scaffolded 501 and owns its own gap issue; where a member of an implemented
//! operation belongs to one of those resources, the comment beside it says so.

use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AvailabilityZone, CacheCluster, CacheNode, CacheParameterGroupStatus, CacheSubnetGroup,
    DestinationDetails, Endpoint, LogDeliveryConfiguration, LogDeliveryConfigurationRequest,
    NotificationConfiguration, PendingModifiedValues, ScaleConfig, SecurityGroupMembership,
    ServiceUpdate, Subnet, Tag,
};

/// The region a request is treated as having been made in when its SigV4
/// credential scope does not name one (an unsigned or anonymous request).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `arnNamespace` / `endpointPrefix` this service's ARNs carry, from the
/// model's `aws.api#service` trait (`smithy_query(query="service_info")`:
/// `arnNamespace: elasticache`).
pub const ARN_NAMESPACE: &str = "elasticache";

/// How a handler words the one condition that leaves it without a store.
///
/// This service's model declares NO server-side error (every modelled error is
/// `errorKind: client`), so there is no 5xx shape to answer with; the synthetic
/// `ValidationException` is used instead of the scaffold's 501, which would read
/// as "this operation is not written yet". The condition itself is unreachable in
/// the assembled emulator: the router builds one `engine::InMemoryEngine` and
/// hands it to every service, and that is the only engine [`service_state`]
/// resolves against.
pub const STORE_UNAVAILABLE: &str =
    "the ElastiCache store is unavailable: this service is mounted on an engine it cannot \
     keep state in";

/// The cache engine a cluster runs when `CreateCacheCluster` names none.
///
/// The model declares no `@default` for `Engine`, so this is the emulator's
/// choice; `redis` is also what the AWS CLI sends when the flag is omitted.
pub const DEFAULT_ENGINE: &str = "redis";

/// The cache engines a cluster may run.
///
/// From the model's own `CreateCacheCluster` documentation: "All nodes in the
/// cluster run the same protocol-compliant cache engine software, either
/// Memcached, Valkey or Redis OSS."
pub const ENGINES: [&str; 3] = ["memcached", "redis", "valkey"];

/// `DescribeCacheClusters`/`DescribeCacheSubnetGroups` `MaxRecords`, as both
/// members document it: "Default: 100. Constraints: minimum 20; maximum 100."
pub const MAX_RECORDS_MIN: i32 = 20;
/// See [`MAX_RECORDS_MIN`].
pub const MAX_RECORDS_MAX: i32 = 100;
/// See [`MAX_RECORDS_MIN`].
pub const MAX_RECORDS_DEFAULT: i32 = 100;

/// The largest Memcached cluster, and the only legal Valkey / Redis OSS node
/// count, as `NumCacheNodes` documents it: "For clusters running Valkey or Redis
/// OSS, this value must be 1. For clusters running Memcached, this value must be
/// between 1 and 40."
pub const MEMCACHED_MAX_NODES: i32 = 40;
/// See [`MEMCACHED_MAX_NODES`].
pub const REDIS_NUM_CACHE_NODES: i32 = 1;

/// The number of nodes a cluster is created with when the request names none.
///
/// The model declares no default; 1 is the only count Valkey / Redis OSS
/// permits and the smallest Memcached cluster, so it is the one value that is
/// legal for every engine.
pub const DEFAULT_NUM_CACHE_NODES: i32 = 1;

/// The `SnapshotRetentionLimit` a Valkey / Redis OSS cluster is created with
/// when the request names none: the member documents "Default: 0 (i.e.,
/// automatic backups are disabled for this cache cluster)". It is "only valid if
/// the Engine parameter is redis", so a Memcached cluster carries none.
pub const DEFAULT_SNAPSHOT_RETENTION_LIMIT: i32 = 0;

/// The weekly maintenance window a cluster is created with when the request
/// names none.
///
/// ElastiCache assigns a random 60-minute window per cluster; the emulator
/// assigns one fixed window instead of inventing a per-cluster random one, and
/// this is the format's own example from `PreferredMaintenanceWindow`
/// ("Example: sun:23:00-mon:01:30").
pub const DEFAULT_MAINTENANCE_WINDOW: &str = "sun:23:00-mon:01:30";

/// How long a cluster stays in the status the call that started the work
/// returned (`creating` after a create, `modifying` after an immediate modify)
/// before the clock owes it `available`.
///
/// ElastiCache takes minutes; the emulator chooses a span, and it is
/// deliberately NOT zero, so a caller acting straight after the call still
/// observes the transient status and the state errors that go with it. The due
/// instant is stored on the record and materialised by whichever later call
/// reads it — nothing sleeps.
pub const CLUSTER_TRANSITION: Duration = Duration::from_millis(500);

/// How many clusters, and how many subnet groups, the store holds before a
/// create is refused with the modelled quota fault.
///
/// ElastiCache publishes adjustable quotas per account per Region and this
/// emulator models neither Service Quotas nor an account's adjustments, so these
/// are the emulator's OWN bounds on the store rather than a claim about any
/// account's limit. They sit far above any workflow an emulator serves, and they
/// are what stops the state growing without end.
pub const CLUSTER_QUOTA: usize = 300;
/// See [`CLUSTER_QUOTA`].
pub const SUBNET_GROUP_QUOTA: usize = 150;

/// How many subnets one cache subnet group may hold.
///
/// The model gives `SubnetIds` no length constraint; this is the emulator's own
/// bound on one group, set at the quota ElastiCache publishes by default, and it
/// is what `CacheSubnetQuotaExceededFault` refuses past.
pub const SUBNETS_PER_GROUP: usize = 20;

/// How many tags a resource may carry, as `TagQuotaPerResourceExceeded`
/// documents it: "The maximum number of tags permitted on a resource is 50."
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// True when `value` satisfies `CacheClusterId`'s documented constraints: "A
/// name must contain from 1 to 50 alphanumeric characters or hyphens. The first
/// character must be a letter. A name cannot end with a hyphen or contain two
/// consecutive hyphens."
///
/// The member carries no modelled `@length` or `@pattern`, so the boundary
/// validator passes any string and the rule is enforced here.
pub fn is_valid_cluster_id(value: &str) -> bool {
    (1..=50).contains(&value.len())
        && value.starts_with(|first: char| first.is_ascii_alphabetic())
        && !value.ends_with('-')
        && !value.contains("--")
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// True when `value` satisfies `CacheSubnetGroupName`'s documented constraint:
/// "Must contain no more than 255 alphanumeric characters or hyphens."
pub fn is_valid_subnet_group_name(value: &str) -> bool {
    (1..=255).contains(&value.len())
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// Why a request's `Tags` cannot be stored.
#[derive(Debug, Clone)]
pub enum TagRefusal {
    /// A tag carried no `Key`. `Tag$Key` is "the key for the tag. May not be
    /// null" — only the VALUE may be.
    MissingKey,
    /// More tags than [`MAX_TAGS_PER_RESOURCE`].
    TooMany(usize),
}

/// The tags a create was given as stored pairs, or why they are refused.
///
/// Order is preserved and duplicate keys are kept as sent, so a later
/// `ListTagsForResource` reports what the caller supplied rather than a set the
/// store quietly collapsed.
pub fn tag_pairs(tags: Option<Vec<Tag>>) -> Result<Vec<(String, String)>, TagRefusal> {
    let tags = tags.unwrap_or_default();
    if tags.len() > MAX_TAGS_PER_RESOURCE {
        return Err(TagRefusal::TooMany(tags.len()));
    }
    tags.into_iter()
        .map(|tag| match tag.key {
            Some(key) => Ok((key, tag.value.unwrap_or_default())),
            None => Err(TagRefusal::MissingKey),
        })
        .collect()
}

/// `CacheClusterStatus` / `CacheNodeStatus` values this slice uses, from
/// `CacheCluster$CacheClusterStatus` ("available, creating, deleted, deleting,
/// incompatible-network, modifying, rebooting cluster nodes, restore-failed, or
/// snapshotting") and `CacheNode$CacheNodeStatus` ("available, creating,
/// rebooting, or deleting").
pub const STATUS_CREATING: &str = "creating";
/// See [`STATUS_CREATING`].
pub const STATUS_AVAILABLE: &str = "available";
/// See [`STATUS_CREATING`].
pub const STATUS_MODIFYING: &str = "modifying";
/// See [`STATUS_CREATING`].
pub const STATUS_DELETING: &str = "deleting";

/// The `ParameterApplyStatus` / `CacheNode$ParameterGroupStatus` of a cluster
/// whose nodes all run the parameter group's current values — the only state
/// this slice can be in, because every parameter-group operation is still a
/// scaffolded 501, so no parameter value can ever be pending.
pub const PARAMETER_APPLY_STATUS: &str = "in-sync";

/// The `SecurityGroupMembership$Status` of a VPC security group a cluster was
/// created with. The emulator models no EC2 security-group lifecycle, so a
/// membership is reported in the one state a created cluster's membership has.
pub const SECURITY_GROUP_STATUS: &str = "active";

/// The `NotificationConfiguration$TopicStatus` of the SNS topic a cluster was
/// given. The emulator models no SNS subscription lifecycle, so a configured
/// topic is reported in the one state a configured topic has.
pub const NOTIFICATION_TOPIC_STATUS: &str = "active";

/// The `LogDeliveryConfiguration$Status` of a log delivery this emulator holds,
/// from `LogDeliveryConfigurationStatus` (`active | enabling | modifying |
/// disabling | error`): a stored configuration is the settled one, and the
/// emulator delivers no logs for it to error on.
pub const LOG_DELIVERY_STATUS: &str = "active";

/// What `CacheCluster$PreferredAvailabilityZone` reports for a cluster whose
/// nodes are not all in one zone: "the name of the Availability Zone in which
/// the cluster is located or "Multiple" if the cache nodes are located in
/// different Availability Zones".
pub const MULTIPLE_AVAILABILITY_ZONES: &str = "Multiple";

/// `AZMode`'s values (`smithy_query(query="shape", shape="AZMode")`:
/// `single-az | cross-az`). `cross-az` is what spreads NEW nodes over zones.
pub const AZ_MODE_CROSS: &str = "cross-az";

/// The engine version a cluster runs when `CreateCacheCluster` names none.
///
/// `DescribeCacheEngineVersions` — the operation that publishes the supported
/// set — is still a scaffolded 501, so these constants are the emulator's single
/// statement of what the current version of each engine is.
pub fn default_engine_version(engine: &str) -> &'static str {
    match engine {
        "memcached" => "1.6.22",
        "valkey" => "8.0",
        _ => "7.1",
    }
}

/// The port a cluster's nodes listen on when `CreateCacheCluster` names none:
/// the engine's own default port (Memcached 11211, Valkey / Redis OSS 6379).
pub fn default_port(engine: &str) -> i32 {
    match engine {
        "memcached" => 11211,
        _ => 6379,
    }
}

/// The parameter group a cluster is associated with when the request names
/// none: "the default parameter group for the specified engine"
/// (`CacheParameterGroupName`).
///
/// An ElastiCache parameter group family is the engine plus its version line —
/// `redis<major>` / `valkey<major>` for Valkey and Redis OSS, and
/// `memcached<major>.<minor>` for Memcached — and the family's default group is
/// named `default.<family>`.
pub fn default_parameter_group_name(engine: &str, engine_version: &str) -> String {
    let mut parts = engine_version.split('.');
    let major = parts.next().unwrap_or_default();
    let minor = parts.next().unwrap_or("0");
    if engine == "memcached" {
        format!("default.memcached{major}.{minor}")
    } else {
        format!("default.{engine}{major}")
    }
}

/// True when `name` names a parameter group this emulator holds.
///
/// Only the engine families' default groups exist: `CreateCacheParameterGroup`
/// is still a scaffolded 501, so no custom group can have been created, and a
/// request naming one is refused with the modelled
/// `CacheParameterGroupNotFoundFault` exactly as it would be against a real
/// account that never created it. A default group is `default.<family>`, and a
/// family is an engine plus its version line — see
/// [`default_parameter_group_name`] — so a name that is not one of those is not
/// a group either.
pub fn is_known_parameter_group(name: &str) -> bool {
    let Some(family) = name.strip_prefix("default.") else {
        return false;
    };
    ENGINES.iter().any(|engine| {
        family.strip_prefix(engine).is_some_and(|version| {
            !version.is_empty() && version.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        })
    })
}

/// The Availability Zone the emulator places the `index`-th subnet of a subnet
/// group, or a cluster that named no `PreferredAvailabilityZone`, in
/// ("Default: System chosen Availability Zone").
///
/// The emulator models no EC2 Availability Zones, so it names them the way AWS
/// does — the region followed by a letter — and spreads a subnet group's subnets
/// over the first few. The chosen zone is STORED on the record, so every later
/// read reports the same one.
pub fn availability_zone(region: &str, index: usize) -> String {
    // Six letters is more zones than any emulated workflow needs and keeps the
    // name inside the `<region><letter>` form AWS uses.
    let letter = char::from(b'a' + u8::try_from(index % 6).unwrap_or(0));
    format!("{region}{letter}")
}

/// True when `value` has the form of an EC2 subnet identifier: `subnet-` and at
/// least one alphanumeric character.
///
/// `SubnetIdentifier` carries no modelled constraint, so the boundary validator
/// passes any string; an id that cannot name a subnet is refused here with the
/// modelled `InvalidSubnet`.
pub fn is_subnet_id(value: &str) -> bool {
    value
        .strip_prefix("subnet-")
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// The ARN of a cache cluster: `arn:aws:elasticache:<region>:<account>:cluster:<id>`.
pub fn cluster_arn(region: &str, cache_cluster_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("cluster:{cache_cluster_id}"))
}

/// The ARN of a cache subnet group:
/// `arn:aws:elasticache:<region>:<account>:subnetgroup:<name>`.
pub fn subnet_group_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("subnetgroup:{name}"))
}

/// The region of the request being served: the third field of its SigV4
/// credential scope, falling back to [`DEFAULT_REGION`] for a request that is
/// not signed.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The pagination `Marker` for a page that starts at `key`.
///
/// Hex of the key rather than the key itself, so a caller cannot read structure
/// into the token and has nothing to build one from: a `Marker` is opaque and
/// only ever travels back unchanged. Encoding the key (rather than an offset)
/// is what makes a page resumable after the records before it changed.
pub fn encode_marker(key: &str) -> String {
    ::hex::encode(key.as_bytes())
}

/// The inclusive start key a marker minted by [`encode_marker`] carries, or
/// `None` when the token was not minted by this service.
pub fn decode_marker(marker: &str) -> Option<String> {
    String::from_utf8(::hex::decode(marker).ok()?).ok()
}

/// The page size a describe serves, or the reason `MaxRecords` is refused.
///
/// `MaxRecords` carries no modelled `@range`, so the boundary validator passes
/// any integer and the documented "minimum 20; maximum 100" is enforced here.
/// It is refused rather than clamped: a caller that asked for a page size the
/// service does not serve gets told so instead of a silently different page.
pub fn page_size(max_records: Option<i32>) -> Result<usize, String> {
    let requested = max_records.unwrap_or(MAX_RECORDS_DEFAULT);
    if !(MAX_RECORDS_MIN..=MAX_RECORDS_MAX).contains(&requested) {
        return Err(format!(
            "Invalid value {requested} for MaxRecords: must be at least {MAX_RECORDS_MIN} and no \
             more than {MAX_RECORDS_MAX}."
        ));
    }
    Ok(usize::try_from(requested).unwrap_or(MAX_RECORDS_MIN as usize))
}

/// The page size a describe whose `MaxRecords` documents no bounds serves, or
/// the reason the requested one is refused.
///
/// `DescribeServiceUpdates$MaxRecords` documents only "The maximum number of
/// records to include in the response" — no "minimum 20; maximum 100" like the
/// cluster and subnet-group describes carry (`smithy_query(query="doc",
/// shape="DescribeServiceUpdatesMessage", member="MaxRecords")`) — so any
/// positive count is a page it serves, and only a count that cannot be one is
/// refused. [`MAX_RECORDS_DEFAULT`] is the page it serves when the request names
/// no `MaxRecords` at all.
pub fn unbounded_page_size(max_records: Option<i32>) -> Result<usize, String> {
    let requested = max_records.unwrap_or(MAX_RECORDS_DEFAULT);
    match usize::try_from(requested) {
        Ok(size) if size > 0 => Ok(size),
        _ => Err(format!(
            "Invalid value {requested} for MaxRecords: must be at least 1."
        )),
    }
}

/// `engine` lowercased, or the reason it names no cache engine.
///
/// ElastiCache stores the engine name in lowercase, so the comparison and the
/// stored value are both lowercase.
pub fn normalize_engine(engine: &str) -> Result<String, String> {
    let lowered = engine.to_ascii_lowercase();
    if ENGINES.contains(&lowered.as_str()) {
        Ok(lowered)
    } else {
        Err(format!(
            "Invalid value {engine} for Engine: must be one of {}.",
            ENGINES.join(" | ")
        ))
    }
}

/// `Ok(())` when `num` is a node count `engine` permits, else the reason it is
/// refused.
///
/// From `NumCacheNodes`: "For clusters running Valkey or Redis OSS, this value
/// must be 1. For clusters running Memcached, this value must be between 1 and
/// 40."
pub fn validate_num_cache_nodes(engine: &str, num: i32) -> Result<(), String> {
    if engine == "memcached" {
        if (1..=MEMCACHED_MAX_NODES).contains(&num) {
            return Ok(());
        }
        return Err(format!(
            "Invalid value {num} for NumCacheNodes: a memcached cluster must have between 1 and \
             {MEMCACHED_MAX_NODES} cache nodes."
        ));
    }
    if num == REDIS_NUM_CACHE_NODES {
        return Ok(());
    }
    Err(format!(
        "Invalid value {num} for NumCacheNodes: a {engine} cluster must have \
         {REDIS_NUM_CACHE_NODES} cache node."
    ))
}

/// One log-delivery configuration of a cluster, as this emulator persists it.
///
/// `LogDeliveryConfigurationRequest` carries an `Enabled` flag; a disabled
/// request removes the configuration rather than storing one, so a stored record
/// is always an enabled one and needs no flag of its own.
#[derive(Debug, Clone)]
pub struct LogDeliveryRecord {
    pub log_type: Option<String>,
    pub destination_type: Option<String>,
    pub destination_details: Option<DestinationDetails>,
    pub log_format: Option<String>,
}

impl LogDeliveryRecord {
    /// This record as the modelled `LogDeliveryConfiguration`.
    pub fn to_modeled(&self) -> LogDeliveryConfiguration {
        LogDeliveryConfiguration {
            log_type: self.log_type.clone(),
            destination_type: self.destination_type.clone(),
            destination_details: self.destination_details.clone(),
            log_format: self.log_format.clone(),
            status: Some(LOG_DELIVERY_STATUS.to_string()),
            // `message` carries the reason a log delivery is in an error state,
            // and a stored configuration is settled.
            message: None,
        }
    }
}

/// Apply `requests` to the log-delivery configurations `stored`: a request
/// replaces the configuration for its log type, and `Enabled: false` removes it.
pub fn apply_log_delivery(
    stored: &mut Vec<LogDeliveryRecord>,
    requests: Vec<LogDeliveryConfigurationRequest>,
) {
    for request in requests {
        stored.retain(|existing| existing.log_type != request.log_type);
        if request.enabled == Some(false) {
            continue;
        }
        stored.push(LogDeliveryRecord {
            log_type: request.log_type,
            destination_type: request.destination_type,
            destination_details: request.destination_details,
            log_format: request.log_format,
        });
    }
}

/// One cache node of a cluster, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct CacheNodeRecord {
    /// A 4-digit numeric identifier (`0001`, `0002`, …), per
    /// `CacheNode$CacheNodeId`.
    pub cache_node_id: String,
    /// One of `CacheNode$CacheNodeStatus`'s values.
    pub status: String,
    pub create_time: SystemTime,
    pub endpoint_address: String,
    pub endpoint_port: i32,
    pub availability_zone: Option<String>,
    pub parameter_group_status: String,
}

impl CacheNodeRecord {
    /// This record as the modelled `CacheNode`.
    ///
    /// A node still being provisioned reports neither endpoint nor creation
    /// time: `DescribeCacheClusters` documents that "if cache nodes are
    /// currently being added to the cluster, node endpoint information and
    /// creation time for the additional nodes are not displayed until they are
    /// completely provisioned", and that a removed node's endpoint is not
    /// displayed either.
    pub fn to_modeled(&self) -> CacheNode {
        let provisioned = self.status == STATUS_AVAILABLE;
        CacheNode {
            cache_node_id: Some(self.cache_node_id.clone()),
            cache_node_status: Some(self.status.clone()),
            cache_node_create_time: provisioned
                .then(|| ::aws_smithy_types::DateTime::from(self.create_time)),
            endpoint: provisioned.then(|| Endpoint {
                address: Some(self.endpoint_address.clone()),
                port: Some(self.endpoint_port),
            }),
            parameter_group_status: Some(self.parameter_group_status.clone()),
            // The primary a read replica syncs from: a replica belongs to a
            // replication group, and every replication-group operation is a
            // scaffolded 501, so a cluster of this slice has no primary.
            source_cache_node_id: None,
            customer_availability_zone: self.availability_zone.clone(),
            // Outposts are not modelled by this slice.
            customer_outpost_arn: None,
        }
    }
}

/// The changes a `ModifyCacheCluster` deferred to the maintenance window: the
/// members `PendingModifiedValues` models, which are exactly the ones
/// `ApplyImmediately` governs. A member that shape does NOT model (the
/// maintenance window, the snapshot settings) has nowhere to be reported as
/// pending, so it is applied at once.
#[derive(Debug, Clone, Default)]
pub struct PendingValues {
    pub num_cache_nodes: Option<i32>,
    pub cache_node_type: Option<String>,
    pub engine_version: Option<String>,
    pub cache_node_ids_to_remove: Vec<String>,
    /// The Memcached scaling configuration. `PendingModifiedValues` is the ONLY
    /// place the model reports a `ScaleConfig`, so a requested one is pending
    /// whatever `ApplyImmediately` says — there is no applied member for it.
    pub scale_config: Option<ScaleConfig>,
}

impl PendingValues {
    /// True when nothing is pending.
    pub fn is_empty(&self) -> bool {
        self.num_cache_nodes.is_none()
            && self.cache_node_type.is_none()
            && self.engine_version.is_none()
            && self.cache_node_ids_to_remove.is_empty()
            && self.scale_config.is_none()
    }

    /// These values as the modelled shape, or `None` when nothing is pending —
    /// a cluster with no pending change reports no member rather than an empty
    /// structure.
    pub fn to_modeled(&self) -> Option<PendingModifiedValues> {
        if self.is_empty() {
            return None;
        }
        Some(PendingModifiedValues {
            num_cache_nodes: self.num_cache_nodes,
            cache_node_ids_to_remove: (!self.cache_node_ids_to_remove.is_empty())
                .then(|| self.cache_node_ids_to_remove.clone()),
            engine_version: self.engine_version.clone(),
            cache_node_type: self.cache_node_type.clone(),
            scale_config: self.scale_config.clone(),
            // An auth token change and a log-delivery change are applied by the
            // handler rather than deferred, so neither is ever pending; the
            // in-transit encryption mode can only be set by
            // `ModifyReplicationGroup`, which is a scaffolded 501.
            auth_token_status: None,
            log_delivery_configurations: None,
            transit_encryption_enabled: None,
            transit_encryption_mode: None,
        })
    }
}

/// A status change the clock owes a cluster: the instant it falls due, and the
/// status the cluster carries from then on.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    pub due: SystemTime,
    pub status: String,
}

/// One cache cluster, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct CacheClusterRecord {
    pub cache_cluster_id: String,
    pub region: String,
    pub engine: String,
    pub engine_version: String,
    pub cache_node_type: Option<String>,
    pub num_cache_nodes: i32,
    /// One of `CacheCluster$CacheClusterStatus`'s values.
    pub status: String,
    /// The status change the clock owes this cluster, if any.
    pub pending_status: Option<PendingTransition>,
    pub created_at: SystemTime,
    /// The DNS label every endpoint of this cluster carries, minted once by
    /// [`ComAmazonawsElasticacheAmazonelasticachev9State::mint_dns_token`] and
    /// stored, so an endpoint is stable across reads.
    pub dns_token: String,
    pub endpoint_address: String,
    pub endpoint_port: i32,
    pub arn: String,
    /// The pool a NEW node's Availability Zone is taken from, in order: one zone
    /// for `single-az`, one per node for `cross-az` or an explicit
    /// `PreferredAvailabilityZones`. A node keeps the zone it was created in —
    /// `AZMode` documents that "existing Memcached nodes remain in their current
    /// Availability Zone. Only newly created nodes are located in different
    /// Availability Zones".
    pub availability_zones: Vec<String>,
    /// The `AZMode` the cluster was created or last modified with, which is what
    /// decides whether the next nodes added to it spread over zones.
    pub az_mode: Option<String>,
    pub preferred_maintenance_window: String,
    pub cache_subnet_group_name: Option<String>,
    /// The replication group this cluster belongs to. Always absent in this
    /// slice: `CreateCacheCluster` refuses a `ReplicationGroupId` while every
    /// replication-group operation is a scaffolded 501, so there is no group to
    /// join. The field is what
    /// `DescribeCacheClusters$ShowCacheClustersNotInReplicationGroups` filters
    /// on, so that filter reads stored state rather than a constant.
    pub replication_group_id: Option<String>,
    pub cache_parameter_group_name: String,
    pub snapshot_retention_limit: Option<i32>,
    pub snapshot_window: Option<String>,
    pub auto_minor_version_upgrade: Option<bool>,
    pub security_group_ids: Vec<String>,
    /// The SNS topic ElastiCache publishes this cluster's events to.
    pub notification_topic_arn: Option<String>,
    /// Whether an `AuthToken` was set. The token itself is a SECRET and is NOT
    /// stored: the model reports only `AuthTokenEnabled` and
    /// `AuthTokenLastModifiedDate`, so those are the only two things kept.
    pub auth_token_enabled: bool,
    /// When the auth token was last set, reported as
    /// `AuthTokenLastModifiedDate`.
    pub auth_token_last_modified: Option<SystemTime>,
    pub transit_encryption_enabled: Option<bool>,
    pub network_type: Option<String>,
    pub ip_discovery: Option<String>,
    pub log_delivery_configurations: Vec<LogDeliveryRecord>,
    pub nodes: Vec<CacheNodeRecord>,
    pub pending_modified_values: PendingValues,
    /// The tags `CreateCacheCluster` was given. `AddTagsToResource` /
    /// `ListTagsForResource` are scaffolded 501s and own their issues; the tags
    /// are persisted here so implementing them does not have to move them.
    pub tags: Vec<(String, String)>,
}

impl CacheClusterRecord {
    /// The configuration endpoint address of this cluster.
    ///
    /// `ConfigurationEndpoint` documents the shape of the name — "The
    /// configuration endpoint will always have `.cfg` in it. Example:
    /// `mem-3.9dvc4r.cfg.usw2.cache.amazonaws.com:11211`" — so the emulator
    /// builds the cluster id, this cluster's stored DNS token, `cfg`, the region
    /// and the service's DNS suffix.
    fn configuration_endpoint_address(cache_cluster_id: &str, dns_token: &str, region: &str) -> String {
        format!("{cache_cluster_id}.{dns_token}.cfg.{region}.cache.amazonaws.com")
    }

    /// The endpoint address of one node, which names the node rather than the
    /// cluster's configuration endpoint.
    fn node_endpoint_address(&self, cache_node_id: &str) -> String {
        format!(
            "{}-{cache_node_id}.{}.{}.cache.amazonaws.com",
            self.cache_cluster_id, self.dns_token, self.region
        )
    }

    /// A node of this cluster with the 4-digit id `ordinal` names, in the status
    /// a just-created node has, placed in `zone`.
    fn new_node(
        &self,
        ordinal: u32,
        now: SystemTime,
        status: &str,
        zone: Option<String>,
    ) -> CacheNodeRecord {
        let cache_node_id = format!("{ordinal:04}");
        CacheNodeRecord {
            endpoint_address: self.node_endpoint_address(&cache_node_id),
            cache_node_id,
            status: status.to_string(),
            create_time: now,
            endpoint_port: self.endpoint_port,
            availability_zone: zone,
            parameter_group_status: PARAMETER_APPLY_STATUS.to_string(),
        }
    }

    /// The Availability Zone the `index`-th node this cluster ever gained is
    /// placed in: the pool, in order, wrapping when there are more nodes than
    /// zones.
    fn zone_for(&self, index: usize) -> Option<String> {
        if self.availability_zones.is_empty() {
            return None;
        }
        Some(self.availability_zones[index % self.availability_zones.len()].clone())
    }

    /// What `CacheCluster$PreferredAvailabilityZone` reports: the zone the
    /// cluster's nodes are in, or [`MULTIPLE_AVAILABILITY_ZONES`] when they are
    /// not all in one.
    pub fn preferred_availability_zone(&self) -> Option<String> {
        let mut zones = self
            .nodes
            .iter()
            .filter_map(|node| node.availability_zone.clone())
            .collect::<Vec<String>>();
        zones.sort();
        zones.dedup();
        match zones.len() {
            0 => self.availability_zones.first().cloned(),
            1 => zones.into_iter().next(),
            _ => Some(MULTIPLE_AVAILABILITY_ZONES.to_string()),
        }
    }

    /// Give this cluster `target` nodes, removing the ids in `ids_to_remove`
    /// first and then, if that is not enough, from the end; a growing cluster
    /// gets new nodes whose ids continue past the highest one in use.
    ///
    /// `new_zones`, when it is not empty, is where the ADDED nodes go, in order
    /// — `ModifyCacheCluster$NewAvailabilityZones` is "the list of Availability
    /// Zones where the new Memcached cache nodes are created". Otherwise the
    /// added nodes take the cluster's own zone pool.
    pub fn resize_nodes(
        &mut self,
        target: i32,
        ids_to_remove: &[String],
        now: SystemTime,
        status: &str,
        new_zones: &[String],
    ) {
        if !ids_to_remove.is_empty() {
            self.nodes
                .retain(|node| !ids_to_remove.contains(&node.cache_node_id));
        }
        let target_len = usize::try_from(target).unwrap_or(0);
        while self.nodes.len() > target_len {
            self.nodes.pop();
        }
        let mut highest = self
            .nodes
            .iter()
            .filter_map(|node| node.cache_node_id.parse::<u32>().ok())
            .max()
            .unwrap_or(0);
        let mut added = 0usize;
        while self.nodes.len() < target_len {
            highest += 1;
            let zone = if new_zones.is_empty() {
                self.zone_for(usize::try_from(highest).unwrap_or(1).saturating_sub(1))
            } else {
                Some(new_zones[added % new_zones.len()].clone())
            };
            let node = self.new_node(highest, now, status, zone);
            self.nodes.push(node);
            added += 1;
        }
        self.num_cache_nodes = target;
    }

    /// Move every node of this cluster to `status`.
    pub fn set_node_status(&mut self, status: &str) {
        for node in &mut self.nodes {
            node.status = status.to_string();
        }
    }

    /// Materialise the status change the clock owes this cluster, if `now` is
    /// past its due instant.
    ///
    /// This is the ONE write a reporting operation may make: the transition was
    /// owed at this instant whether or not anybody called. Nothing else about
    /// the cluster moves.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(pending) = self.pending_status.as_ref() else {
            return;
        };
        if now < pending.due {
            return;
        }
        self.status = pending.status.clone();
        self.pending_status = None;
        if self.status == STATUS_AVAILABLE {
            self.set_node_status(STATUS_AVAILABLE);
        }
    }

    /// Start the span after which the clock owes this cluster `available`,
    /// leaving it in `status` until then.
    pub fn begin_transition(&mut self, status: &str, now: SystemTime) {
        self.status = status.to_string();
        self.pending_status = Some(PendingTransition {
            due: now + CLUSTER_TRANSITION,
            status: STATUS_AVAILABLE.to_string(),
        });
    }

    /// Whether a describe reports this cluster's node list at all.
    ///
    /// `DescribeCacheClusters`: "If the cluster is in the creating state, only
    /// cluster-level information is displayed until all of the nodes are
    /// successfully provisioned. If the cluster is in the deleting state, only
    /// cluster-level information is displayed."
    pub fn reports_node_detail(&self) -> bool {
        self.status != STATUS_CREATING && self.status != STATUS_DELETING
    }

    /// This record as the modelled `CacheCluster`.
    ///
    /// `CacheNodes` is populated only when the caller asked for it
    /// (`DescribeCacheClusters$ShowCacheNodeInfo`) AND the cluster is past the
    /// states that report cluster-level information only; see
    /// [`CacheClusterRecord::reports_node_detail`]. `include_nodes` is `true`
    /// for the create / modify / delete responses, which carry the cluster's
    /// own node list.
    pub fn to_modeled(&self, include_nodes: bool) -> CacheCluster {
        CacheCluster {
            cache_cluster_id: Some(self.cache_cluster_id.clone()),
            configuration_endpoint: Some(Endpoint {
                address: Some(self.endpoint_address.clone()),
                port: Some(self.endpoint_port),
            }),
            // The client-library download page is a console URL, not state this
            // emulator holds.
            client_download_landing_page: None,
            cache_node_type: self.cache_node_type.clone(),
            engine: Some(self.engine.clone()),
            engine_version: Some(self.engine_version.clone()),
            cache_cluster_status: Some(self.status.clone()),
            num_cache_nodes: Some(self.num_cache_nodes),
            preferred_availability_zone: self.preferred_availability_zone(),
            // Outposts: `CreateCacheCluster` refuses every Outpost member,
            // because this emulator provisions no Outpost capacity to place a
            // cluster in.
            preferred_outpost_arn: None,
            cache_cluster_create_time: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            preferred_maintenance_window: Some(self.preferred_maintenance_window.clone()),
            pending_modified_values: self.pending_modified_values.to_modeled(),
            notification_configuration: self.notification_topic_arn.as_ref().map(|arn| {
                NotificationConfiguration {
                    topic_arn: Some(arn.clone()),
                    topic_status: Some(NOTIFICATION_TOPIC_STATUS.to_string()),
                }
            }),
            // Cache security groups (the pre-VPC, EC2-Classic mechanism) cannot
            // exist here: `CreateCacheSecurityGroup` is a scaffolded 501.
            cache_security_groups: None,
            cache_parameter_group: Some(CacheParameterGroupStatus {
                cache_parameter_group_name: Some(self.cache_parameter_group_name.clone()),
                parameter_apply_status: Some(PARAMETER_APPLY_STATUS.to_string()),
                // No parameter change can be pending, so no node needs a reboot
                // for one.
                cache_node_ids_to_reboot: None,
            }),
            cache_subnet_group_name: self.cache_subnet_group_name.clone(),
            cache_nodes: (include_nodes && self.reports_node_detail())
                .then(|| self.nodes.iter().map(CacheNodeRecord::to_modeled).collect()),
            auto_minor_version_upgrade: self.auto_minor_version_upgrade,
            security_groups: (!self.security_group_ids.is_empty()).then(|| {
                self.security_group_ids
                    .iter()
                    .map(|id| SecurityGroupMembership {
                        security_group_id: Some(id.clone()),
                        status: Some(SECURITY_GROUP_STATUS.to_string()),
                    })
                    .collect()
            }),
            // A cluster of this slice belongs to no replication group: every
            // replication-group operation is a scaffolded 501, so
            // `CreateCacheCluster` refuses a `ReplicationGroupId`.
            replication_group_id: self.replication_group_id.clone(),
            snapshot_retention_limit: self.snapshot_retention_limit,
            snapshot_window: self.snapshot_window.clone(),
            auth_token_enabled: Some(self.auth_token_enabled),
            auth_token_last_modified_date: self
                .auth_token_last_modified
                .map(::aws_smithy_types::DateTime::from),
            transit_encryption_enabled: self.transit_encryption_enabled,
            // At-rest encryption "cannot be modified after the cluster is
            // created" and `CreateCacheCluster` has no member to set it, so no
            // cluster of this API can carry it.
            at_rest_encryption_enabled: None,
            arn: Some(self.arn.clone()),
            // Log delivery "for the replication group": a cluster of this slice
            // belongs to none.
            replication_group_log_delivery_enabled: None,
            log_delivery_configurations: (!self.log_delivery_configurations.is_empty()).then(|| {
                self.log_delivery_configurations
                    .iter()
                    .map(LogDeliveryRecord::to_modeled)
                    .collect()
            }),
            network_type: self.network_type.clone(),
            ip_discovery: self.ip_discovery.clone(),
            // The in-transit encryption MODE is set by
            // `ModifyReplicationGroup`, a scaffolded 501.
            transit_encryption_mode: None,
        }
    }
}

/// One subnet of a cache subnet group, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct SubnetRecord {
    pub subnet_identifier: String,
    /// The Availability Zone chosen for this subnet when the group was created
    /// and stored since, so every read reports the same one.
    pub availability_zone: String,
}

/// One cache subnet group, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct CacheSubnetGroupRecord {
    pub name: String,
    pub description: String,
    pub subnets: Vec<SubnetRecord>,
    /// The VPC the group's subnets belong to, minted once by
    /// [`ComAmazonawsElasticacheAmazonelasticachev9State::mint_vpc_id`] because
    /// the emulator models no EC2 VPCs to read one from.
    pub vpc_id: String,
    pub arn: String,
    /// The tags `CreateCacheSubnetGroup` was given; see
    /// [`CacheClusterRecord::tags`].
    pub tags: Vec<(String, String)>,
}

impl CacheSubnetGroupRecord {
    /// This record as the modelled `CacheSubnetGroup`.
    pub fn to_modeled(&self) -> CacheSubnetGroup {
        CacheSubnetGroup {
            cache_subnet_group_name: Some(self.name.clone()),
            cache_subnet_group_description: Some(self.description.clone()),
            vpc_id: Some(self.vpc_id.clone()),
            subnets: Some(
                self.subnets
                    .iter()
                    .map(|subnet| Subnet {
                        subnet_identifier: Some(subnet.subnet_identifier.clone()),
                        subnet_availability_zone: Some(AvailabilityZone {
                            name: Some(subnet.availability_zone.clone()),
                        }),
                        subnet_outpost: None,
                        supported_network_types: None,
                    })
                    .collect(),
            ),
            arn: Some(self.arn.clone()),
            supported_network_types: None,
        }
    }
}

/// `ServiceUpdateStatus`'s values (`smithy_query(query="shape",
/// shape="ServiceUpdateStatus")`: `available | cancelled | expired`). A filter
/// naming anything else is refused by the boundary validator before a handler
/// runs, because the member is a modelled enum.
pub const SERVICE_UPDATE_AVAILABLE: &str = "available";
/// See [`SERVICE_UPDATE_AVAILABLE`].
pub const SERVICE_UPDATE_CANCELLED: &str = "cancelled";
/// See [`SERVICE_UPDATE_AVAILABLE`].
pub const SERVICE_UPDATE_EXPIRED: &str = "expired";

/// The only value `ServiceUpdateType` declares
/// (`smithy_query(query="shape", shape="ServiceUpdateType")`:
/// `security-update`).
pub const SERVICE_UPDATE_TYPE: &str = "security-update";

/// The `EstimatedUpdateTime` every published update carries: "The estimated
/// length of time the service update will take".
///
/// Nothing in the model or the docs states what that length is for a given
/// update, so the emulator publishes one estimate for all of them rather than
/// inventing a per-update one.
pub const SERVICE_UPDATE_ESTIMATED_TIME: &str = "15 minutes";

/// A day, the unit the published catalog's dates are spaced by.
pub const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// One row of the service-update catalog this emulator publishes, as offsets in
/// whole days from the instant the store came up.
///
/// AWS publishes ElastiCache service updates itself — the API has no operation
/// that creates, cancels or expires one — and neither the model nor the docs
/// name any particular update, so this table is the emulator's OWN published
/// catalog rather than a claim about what AWS has released. The dates are
/// relative to the store's own anchor so the catalog keeps the shape a real
/// account sees (a recent update still inside its availability window, and an
/// earlier one the clock has already carried past its end date) however long
/// after this build the emulator runs.
struct PublishedUpdate {
    /// One of `ServiceUpdateSeverity`'s values (`critical | important | medium |
    /// low`).
    severity: &'static str,
    /// How long before the anchor the update was released.
    released_days_before: u64,
    /// How long after its release the update is recommended to be applied by.
    apply_by_days_after_release: u64,
    /// How long after its release the update stops being available.
    end_days_after_release: u64,
    /// `AutoUpdateAfterRecommendedApplyByDate`: "whether the service update
    /// will be automatically applied once the recommended apply-by date has
    /// expired".
    auto_update_after_recommended_apply_by_date: bool,
}

/// The catalog's rows. Every engine gets one update per row, so the set spans
/// both the availability window a published update is inside and the one it has
/// fallen out of.
const PUBLISHED_UPDATES: [PublishedUpdate; 2] = [
    PublishedUpdate {
        severity: "critical",
        released_days_before: 180,
        apply_by_days_after_release: 30,
        end_days_after_release: 120,
        auto_update_after_recommended_apply_by_date: true,
    },
    PublishedUpdate {
        severity: "important",
        released_days_before: 7,
        apply_by_days_after_release: 30,
        end_days_after_release: 120,
        auto_update_after_recommended_apply_by_date: false,
    },
];

/// One published service update, as this emulator holds it.
///
/// `ServiceUpdateStatus` is NOT stored: it follows from the update's dates and
/// the clock, so [`ServiceUpdateRecord::status`] derives it on every read
/// instead of letting a stored copy drift from the dates beside it.
#[derive(Debug, Clone)]
pub struct ServiceUpdateRecord {
    /// "The unique ID of the service update", and the identity
    /// `ServiceUpdateName` names it by.
    pub service_update_name: String,
    /// "The Elasticache engine to which the update applies."
    pub engine: String,
    /// "The Elasticache engine version to which the update applies."
    pub engine_version: String,
    /// One of `ServiceUpdateSeverity`'s values.
    pub severity: String,
    /// One of `ServiceUpdateType`'s values — see [`SERVICE_UPDATE_TYPE`].
    pub update_type: String,
    /// "Provides details of the service update."
    pub description: String,
    /// "The date when the service update is initially available."
    pub release_date: SystemTime,
    /// "The recommendend date to apply the service update in order to ensure
    /// compliance."
    pub recommended_apply_by_date: SystemTime,
    /// "The date after which the service update is no longer available."
    pub end_date: SystemTime,
    /// See [`PublishedUpdate::auto_update_after_recommended_apply_by_date`].
    pub auto_update_after_recommended_apply_by_date: bool,
    /// See [`SERVICE_UPDATE_ESTIMATED_TIME`].
    pub estimated_update_time: String,
}

impl ServiceUpdateRecord {
    /// This update's `ServiceUpdateStatus` at `now`.
    ///
    /// `ServiceUpdateEndDate` is "the date after which the service update is no
    /// longer available", so an update the clock has carried past it is
    /// `expired` and one still inside its window is `available`. The third
    /// status, `cancelled`, is the status of an update AWS withdrew before its
    /// end date; this emulator publishes none, and no operation in this service
    /// withdraws one, so no record ever reports it.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now >= self.end_date {
            SERVICE_UPDATE_EXPIRED
        } else {
            SERVICE_UPDATE_AVAILABLE
        }
    }

    /// This record as the modelled `ServiceUpdate`, with its status taken from
    /// the clock at `now`.
    pub fn to_modeled(&self, now: SystemTime) -> ServiceUpdate {
        ServiceUpdate {
            service_update_name: Some(self.service_update_name.clone()),
            service_update_release_date: Some(::aws_smithy_types::DateTime::from(self.release_date)),
            service_update_end_date: Some(::aws_smithy_types::DateTime::from(self.end_date)),
            service_update_severity: Some(self.severity.clone()),
            service_update_recommended_apply_by_date: Some(::aws_smithy_types::DateTime::from(
                self.recommended_apply_by_date,
            )),
            service_update_status: Some(self.status(now).to_string()),
            service_update_description: Some(self.description.clone()),
            service_update_type: Some(self.update_type.clone()),
            engine: Some(self.engine.clone()),
            engine_version: Some(self.engine_version.clone()),
            auto_update_after_recommended_apply_by_date: Some(
                self.auto_update_after_recommended_apply_by_date,
            ),
            estimated_update_time: Some(self.estimated_update_time.clone()),
        }
    }
}

/// The `YYYYMMDD` an update released at `at` is named after.
///
/// Built from the RFC-3339 rendering of the instant, so the service's one
/// timestamp implementation is also what dates a name.
fn release_date_label(at: SystemTime) -> String {
    ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .map(|rendered| {
            rendered
                .chars()
                .take_while(|c| *c != 'T')
                .filter(|c| c.is_ascii_digit())
                .collect()
        })
        // Unreachable for an instant this century; an unrenderable date is
        // simply not part of a name.
        .unwrap_or_default()
}

/// The service-update catalog as of `anchor`, keyed by `ServiceUpdateName`.
///
/// One update per engine per row of [`PUBLISHED_UPDATES`], named the way AWS
/// names an ElastiCache service update — `elasticache-<YYYYMMDD>-<NNN>`, the
/// release date and a sequence number within that date. A `BTreeMap` is what
/// gives `DescribeServiceUpdates` the stable total order its `Marker` indexes
/// into.
pub fn published_service_updates(anchor: SystemTime) -> BTreeMap<String, ServiceUpdateRecord> {
    let mut catalog = BTreeMap::new();
    for row in &PUBLISHED_UPDATES {
        let release_date = anchor
            .checked_sub(DAY * u32::try_from(row.released_days_before).unwrap_or(u32::MAX))
            .unwrap_or(anchor);
        let label = release_date_label(release_date);
        for (index, engine) in ENGINES.iter().enumerate() {
            let name = format!("elasticache-{label}-{:03}", index + 1);
            let engine_version = default_engine_version(engine);
            let severity = row.severity;
            catalog.insert(
                name.clone(),
                ServiceUpdateRecord {
                    service_update_name: name,
                    engine: (*engine).to_string(),
                    engine_version: engine_version.to_string(),
                    severity: severity.to_string(),
                    update_type: SERVICE_UPDATE_TYPE.to_string(),
                    description: format!(
                        "A {severity} security update for {engine} {engine_version}."
                    ),
                    release_date,
                    recommended_apply_by_date: release_date
                        + DAY * u32::try_from(row.apply_by_days_after_release).unwrap_or(u32::MAX),
                    end_date: release_date
                        + DAY * u32::try_from(row.end_days_after_release).unwrap_or(u32::MAX),
                    auto_update_after_recommended_apply_by_date: row
                        .auto_update_after_recommended_apply_by_date,
                    estimated_update_time: SERVICE_UPDATE_ESTIMATED_TIME.to_string(),
                },
            );
        }
    }
    catalog
}

/// Per-service state for com-amazonaws-elasticache-amazonelasticachev9. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsElasticacheAmazonelasticachev9State {
    /// Every stored cluster, keyed by its (lowercased) `CacheClusterId` — the
    /// resource's identity in this API. A `BTreeMap` is what gives
    /// `DescribeCacheClusters` the stable total order its `Marker` indexes into.
    pub cache_clusters: BTreeMap<String, CacheClusterRecord>,
    /// Every stored subnet group, keyed by its (lowercased) name, ordered for
    /// the same reason.
    pub cache_subnet_groups: BTreeMap<String, CacheSubnetGroupRecord>,
    /// The service updates this emulator publishes, keyed by
    /// `ServiceUpdateName` and ordered for the same reason. AWS releases these
    /// rather than an account creating them, so the store holds the catalog
    /// from the moment it comes up — see [`published_service_updates`].
    pub service_updates: BTreeMap<String, ServiceUpdateRecord>,
    /// The counter endpoint DNS labels are minted from.
    pub dns_token_seq: u64,
    /// The counter VPC ids are minted from.
    pub vpc_seq: u64,
}

impl Default for ComAmazonawsElasticacheAmazonelasticachev9State {
    /// A store with no customer resources in it, already holding the published
    /// service-update catalog.
    ///
    /// The engine builds this the first time a handler of this service asks for
    /// its state, so that instant is the anchor the catalog's release and end
    /// dates hang off — fixed for the life of the process, which is what makes
    /// every later read of an update report the same dates.
    fn default() -> Self {
        Self {
            cache_clusters: BTreeMap::new(),
            cache_subnet_groups: BTreeMap::new(),
            service_updates: published_service_updates(SystemTime::now()),
            dns_token_seq: 0,
            vpc_seq: 0,
        }
    }
}

impl ComAmazonawsElasticacheAmazonelasticachev9State {
    /// Mint the DNS label a new cluster's endpoints carry: six lowercase
    /// hex digits, the shape of the label in `ConfigurationEndpoint`'s example.
    pub fn mint_dns_token(&mut self) -> String {
        self.dns_token_seq = self.dns_token_seq.wrapping_add(1);
        format!("{:06x}", self.dns_token_seq & 0xff_ffff)
    }

    /// Mint the VPC id a new subnet group reports: `vpc-` and eight lowercase
    /// hex digits, the form EC2 uses.
    pub fn mint_vpc_id(&mut self) -> String {
        self.vpc_seq = self.vpc_seq.wrapping_add(1);
        format!("vpc-{:08x}", self.vpc_seq & 0xffff_ffff)
    }

    /// Build and store a cluster record, returning what was stored.
    ///
    /// Takes the already-validated values so the one place a cluster is created
    /// is also the one place its endpoints, ARN and nodes are derived from them.
    pub fn insert_cluster(&mut self, new: NewCluster) -> &CacheClusterRecord {
        let dns_token = self.mint_dns_token();
        let endpoint_address = CacheClusterRecord::configuration_endpoint_address(
            &new.cache_cluster_id,
            &dns_token,
            &new.region,
        );
        let mut record = CacheClusterRecord {
            arn: cluster_arn(&new.region, &new.cache_cluster_id),
            cache_cluster_id: new.cache_cluster_id.clone(),
            region: new.region,
            engine: new.engine,
            engine_version: new.engine_version,
            cache_node_type: new.cache_node_type,
            num_cache_nodes: 0,
            status: STATUS_CREATING.to_string(),
            pending_status: Some(PendingTransition {
                due: new.now + CLUSTER_TRANSITION,
                status: STATUS_AVAILABLE.to_string(),
            }),
            created_at: new.now,
            dns_token,
            endpoint_address,
            endpoint_port: new.port,
            availability_zones: new.availability_zones,
            az_mode: new.az_mode,
            preferred_maintenance_window: new.preferred_maintenance_window,
            cache_subnet_group_name: new.cache_subnet_group_name,
            replication_group_id: None,
            cache_parameter_group_name: new.cache_parameter_group_name,
            snapshot_retention_limit: new.snapshot_retention_limit,
            snapshot_window: new.snapshot_window,
            auto_minor_version_upgrade: new.auto_minor_version_upgrade,
            security_group_ids: new.security_group_ids,
            notification_topic_arn: new.notification_topic_arn,
            auth_token_enabled: new.auth_token_enabled,
            auth_token_last_modified: new.auth_token_enabled.then_some(new.now),
            transit_encryption_enabled: new.transit_encryption_enabled,
            network_type: new.network_type,
            ip_discovery: new.ip_discovery,
            log_delivery_configurations: new.log_delivery_configurations,
            nodes: Vec::new(),
            pending_modified_values: PendingValues::default(),
            tags: new.tags,
        };
        record.resize_nodes(new.num_cache_nodes, &[], new.now, STATUS_CREATING, &[]);
        // The response is read back out of the map, so it is built from the
        // record this service persisted rather than from a copy beside it.
        match self.cache_clusters.entry(new.cache_cluster_id) {
            Entry::Vacant(slot) => slot.insert(record),
            Entry::Occupied(mut slot) => {
                slot.insert(record);
                slot.into_mut()
            }
        }
    }

    /// The cluster `id` names, with any status change the clock owes it already
    /// materialised.
    pub fn settled_cluster_mut(
        &mut self,
        id: &str,
        now: SystemTime,
    ) -> Option<&mut CacheClusterRecord> {
        let cluster = self.cache_clusters.get_mut(id)?;
        cluster.settle(now);
        Some(cluster)
    }

    /// Materialise every status change the clock owes, for a caller that is
    /// about to report on all of them.
    pub fn settle_all(&mut self, now: SystemTime) {
        for cluster in self.cache_clusters.values_mut() {
            cluster.settle(now);
        }
    }
}

/// The validated values a new cluster is built from — one argument so
/// [`ComAmazonawsElasticacheAmazonelasticachev9State::insert_cluster`] does not
/// take a dozen positional strings.
#[derive(Debug, Clone)]
pub struct NewCluster {
    pub cache_cluster_id: String,
    pub region: String,
    pub engine: String,
    pub engine_version: String,
    pub cache_node_type: Option<String>,
    pub num_cache_nodes: i32,
    pub port: i32,
    pub availability_zones: Vec<String>,
    pub az_mode: Option<String>,
    pub preferred_maintenance_window: String,
    pub cache_subnet_group_name: Option<String>,
    pub cache_parameter_group_name: String,
    pub snapshot_retention_limit: Option<i32>,
    pub snapshot_window: Option<String>,
    pub auto_minor_version_upgrade: Option<bool>,
    pub security_group_ids: Vec<String>,
    pub notification_topic_arn: Option<String>,
    pub auth_token_enabled: bool,
    pub transit_encryption_enabled: Option<bool>,
    pub network_type: Option<String>,
    pub ip_discovery: Option<String>,
    pub log_delivery_configurations: Vec<LogDeliveryRecord>,
    pub tags: Vec<(String, String)>,
    pub now: SystemTime,
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsElasticacheAmazonelasticachev9State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsElasticacheAmazonelasticachev9State>)
}
