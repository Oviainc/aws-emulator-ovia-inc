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
//! The serverless cache is ElastiCache's other cache resource:
//! `CreateServerlessCache` mints one and `DescribeServerlessCaches` reports it.
//! Its record and its projection into the modelled `ServerlessCache` shape live
//! here too, so `ModifyServerlessCache` / `DeleteServerlessCache` and the
//! serverless snapshot operations — still scaffolded 501s, each with its own gap
//! issue — write the same copy the reader reads instead of introducing a second
//! one.
//!
//! A snapshot is a copy of a cluster as of the time it was taken:
//! `CreateSnapshot` mints one from a stored cluster and `DescribeSnapshots`
//! reports it, so [`SnapshotRecord`] below is the one copy both read.
//!
//! Beside those customer-created resources sit the two catalogs this service
//! publishes rather than mints: the service updates AWS releases for its cache
//! engines, and the cache engines and versions themselves. No operation creates
//! either (the API has no such call), so the store starts out holding them and
//! `DescribeServiceUpdates` / `DescribeCacheEngineVersions` report them. The
//! engine-version catalog is also the one statement of which version is the
//! newest of each engine and which parameter group family a version belongs to,
//! so `CreateCacheCluster`'s defaults are read off it rather than from a second
//! list.
//!
//! The reserved-cache-node offerings are a further such catalog, and the
//! reservations bought from one ([`ReservedCacheNodeRecord`]) ARE an account
//! resource: `PurchaseReservedCacheNodesOffering` mints one and
//! `DescribeReservedCacheNodes` reports it.
//!
//! Beside the cache resources sits the RBAC user: `CreateUser` mints one and
//! `DescribeUsers` reports it, over the one [`UserRecord`] map below, which also
//! holds the built-in `default` user every account already has.
//!
//! Everything else in this service (user groups, parameter groups, migrations,
//! update actions) is still a scaffolded 501 and owns its own gap issue; where a
//! member of an implemented operation belongs to one of those resources, the
//! comment beside it says so.
//!
//! The serverless cache is another customer-created resource this store holds:
//! `CreateServerlessCache` mints one and `DescribeServerlessCaches` reports it.
//! `ModifyServerlessCache` / `DeleteServerlessCache` and the serverless
//! snapshot operations are still scaffolded 501s with their own gap issues, and
//! they read and write the SAME record rather than a copy beside it.
//!
//! A replication group is the other resource this state holds:
//! `CreateReplicationGroup` mints one together with the member cache clusters it
//! is made of, and `DescribeReplicationGroups` reports it. Those member clusters
//! are ordinary [`CacheClusterRecord`]s in the same map as the standalone ones,
//! so there is one copy of a cluster however it was created.

use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Authentication, AuthenticationMode, AvailabilityZone, CacheCluster, CacheEngineVersion,
    CacheNode, CacheParameterGroup, CacheParameterGroupStatus, CacheSubnetGroup, CacheUsageLimits,
    CreateUserOutput, DestinationDetails, Endpoint, LogDeliveryConfiguration,
    LogDeliveryConfigurationRequest, NodeGroup, NodeGroupConfiguration, NodeGroupMember,
    NodeSnapshot, NotificationConfiguration, PendingModifiedValues, RecurringCharge,
    ReplicationGroup, ReservedCacheNode, ReservedCacheNodesOffering, ScaleConfig,
    SecurityGroupMembership, ServerlessCache, ServiceUpdate, Snapshot, Subnet, Tag,
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

/// The Valkey engine, named wherever behaviour differs by engine (the version
/// catalogue, a replication group's at-rest encryption default).
pub const ENGINE_VALKEY: &str = "valkey";

/// The cache engines a cluster may run.
///
/// From the model's own `CreateCacheCluster` documentation: "All nodes in the
/// cluster run the same protocol-compliant cache engine software, either
/// Memcached, Valkey or Redis OSS."
pub const ENGINES: [&str; 3] = ["memcached", DEFAULT_ENGINE, ENGINE_VALKEY];

/// The `MaxRecords` of every paginated describe in this service, as each of
/// those members documents it: "Default: 100. Constraints: minimum 20; maximum
/// 100."
pub const MAX_RECORDS_MIN: i32 = 20;
/// See [`MAX_RECORDS_MIN`].
pub const MAX_RECORDS_MAX: i32 = 100;
/// See [`MAX_RECORDS_MIN`].
pub const MAX_RECORDS_DEFAULT: i32 = 100;

/// `DescribeSnapshots$MaxRecords`, as that member documents it: "Default: 50.
/// Constraints: minimum 20; maximum 50." The minimum is the same
/// [`MAX_RECORDS_MIN`] every paginated describe in this service documents; the
/// default and the maximum are this operation's own.
pub const SNAPSHOT_MAX_RECORDS_MAX: i32 = 50;
/// See [`SNAPSHOT_MAX_RECORDS_MAX`].
pub const SNAPSHOT_MAX_RECORDS_DEFAULT: i32 = 50;

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

/// How long a snapshot stays `creating` before the clock owes it `available`.
///
/// The same reasoning as [`CLUSTER_TRANSITION`]: ElastiCache takes minutes, the
/// emulator chooses a span, and it is deliberately NOT zero so a caller acting
/// straight after `CreateSnapshot` still observes the transient status.
pub const SNAPSHOT_TRANSITION: Duration = Duration::from_millis(500);

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
/// See [`CLUSTER_QUOTA`]; this is what `SnapshotQuotaExceededFault` refuses
/// past.
pub const SNAPSHOT_QUOTA: usize = 300;

/// How many subnets one cache subnet group may hold.
///
/// The model gives `SubnetIds` no length constraint; this is the emulator's own
/// bound on one group, set at the quota ElastiCache publishes by default, and it
/// is what `CacheSubnetQuotaExceededFault` refuses past.
pub const SUBNETS_PER_GROUP: usize = 20;

/// How many tags a resource may carry, as `TagQuotaPerResourceExceeded`
/// documents it: "The maximum number of tags permitted on a resource is 50."
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The longest `CacheClusterId`: "A name must contain from 1 to 50 alphanumeric
/// characters or hyphens."
pub const CLUSTER_ID_MAX_LEN: usize = 50;

/// The longest `ReplicationGroupId`: "A name must contain from 1 to 40
/// alphanumeric characters or hyphens."
pub const REPLICATION_GROUP_ID_MAX_LEN: usize = 40;

/// True when `value` satisfies the name rule ElastiCache documents for every one
/// of its identifiers — "A name must contain from 1 to `max_len` alphanumeric
/// characters or hyphens. The first character must be a letter. A name cannot
/// end with a hyphen or contain two consecutive hyphens." — the only thing that
/// differs between them being the length bound.
///
/// These members carry no modelled `@length` or `@pattern`, so the boundary
/// validator passes any string and the rule is enforced here.
pub fn is_valid_identifier(value: &str, max_len: usize) -> bool {
    (1..=max_len).contains(&value.len())
        && value.starts_with(|first: char| first.is_ascii_alphabetic())
        && !value.ends_with('-')
        && !value.contains("--")
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// True when `value` satisfies `CacheClusterId`'s documented constraints; see
/// [`is_valid_identifier`].
pub fn is_valid_cluster_id(value: &str) -> bool {
    is_valid_identifier(value, CLUSTER_ID_MAX_LEN)
}

/// True when `value` satisfies `ReplicationGroupId`'s documented constraints;
/// see [`is_valid_identifier`].
pub fn is_valid_replication_group_id(value: &str) -> bool {
    is_valid_identifier(value, REPLICATION_GROUP_ID_MAX_LEN)
}

/// True when `value` can name a snapshot: 1 to 255 alphanumeric characters or
/// hyphens.
///
/// `SnapshotName` carries no modelled `@length` or `@pattern` and the model
/// documents only that the value "is stored as a lowercase string", so the only
/// form enforced here is the character set and the length bound this service's
/// names document (see [`is_valid_subnet_group_name`]: "must contain no more
/// than 255 alphanumeric characters or hyphens"). Nothing refuses a hyphen's
/// placement or a leading digit, because no ElastiCache documentation states
/// either of a snapshot name and a name AWS accepts must not be refused here.
pub fn is_valid_snapshot_name(value: &str) -> bool {
    (1..=255).contains(&value.len())
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
/// See [`STATUS_CREATING`]: the status a cluster carries while a snapshot of it
/// is being taken.
pub const STATUS_SNAPSHOTTING: &str = "snapshotting";

/// What `Snapshot$SnapshotSource` reports — "whether the snapshot is from an
/// automatic backup (`automated`) or was created manually (`manual`)".
///
/// `CreateSnapshot` is the manual path. No `automated` snapshot is ever stored:
/// ElastiCache takes one during the source cluster's daily `SnapshotWindow`, so
/// the first one falls due a day after the cluster was created, and a cluster
/// whose `SnapshotRetentionLimit` is 0 has "automatic backups disabled" and is
/// never owed one at all.
pub const SNAPSHOT_SOURCE_MANUAL: &str = "manual";
/// See [`SNAPSHOT_SOURCE_MANUAL`].
pub const SNAPSHOT_SOURCE_AUTOMATED: &str = "automated";

/// The values `DescribeSnapshots$SnapshotSource` filters on: "If set to
/// `system`, the output shows snapshots that were automatically created by
/// ElastiCache. If set to `user` the output shows snapshots that were manually
/// created." They are the FILTER's spellings, not the ones
/// [`SNAPSHOT_SOURCE_MANUAL`] / [`SNAPSHOT_SOURCE_AUTOMATED`] are reported
/// under.
pub const SNAPSHOT_SOURCE_FILTER_SYSTEM: &str = "system";
/// See [`SNAPSHOT_SOURCE_FILTER_SYSTEM`].
pub const SNAPSHOT_SOURCE_FILTER_USER: &str = "user";

/// The node type ElastiCache cannot snapshot a Valkey / Redis OSS cluster on,
/// from `SnapshotFeatureNotSupportedFault`: "Creating a snapshot of a Valkey or
/// Redis OSS cluster running on a `cache.t1.micro` cache node" is not
/// supported.
pub const UNSNAPSHOTTABLE_NODE_TYPE: &str = "cache.t1.micro";

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

/// The engine version a cluster runs when `CreateCacheCluster` names none: the
/// newest version this service publishes for that engine.
///
/// Read out of the ONE catalog `DescribeCacheEngineVersions` reports
/// ([`CACHE_ENGINE_VERSIONS`]) rather than from a second list beside it, so a
/// cluster is never created on a version the service does not say it supports.
/// An engine the catalog publishes nothing for falls back to
/// [`DEFAULT_ENGINE`]'s newest version, which is unreachable for the engines in
/// [`ENGINES`] — the only ones a request gets past [`normalize_engine`] with.
pub fn default_engine_version(engine: &str) -> &'static str {
    latest_published_version(engine)
        .or_else(|| latest_published_version(DEFAULT_ENGINE))
        .unwrap_or_default()
}

/// The port a cluster's nodes listen on when `CreateCacheCluster` names none:
/// the engine's own default port (Memcached 11211, Valkey / Redis OSS 6379).
pub fn default_port(engine: &str) -> i32 {
    match engine {
        "memcached" => 11211,
        _ => 6379,
    }
}

/// Every cache parameter group family this service publishes.
///
/// The order here is the catalog's own; what a describe pages is the STORED key
/// order of
/// [`ComAmazonawsElasticacheAmazonelasticachev9State::cache_parameter_groups`].
///
/// Source: `smithy_query(query="doc", shape="CreateCacheParameterGroupMessage",
/// member="CacheParameterGroupFamily")` — "Valid values are: valkey8 | valkey7
/// | memcached1.4 | memcached1.5 | memcached1.6 | redis2.6 | redis2.8 |
/// redis3.2 | redis4.0 | redis5.0 | redis6.x | redis7". A family's name is the
/// engine followed by its version line, which is why the line is `<major>` for
/// Valkey and current Redis OSS, `<major>.<minor>` for Memcached and the older
/// Redis OSS lines, and `6.x` for Redis OSS 6.
pub const PARAMETER_GROUP_FAMILIES: [&str; 12] = [
    "memcached1.4",
    "memcached1.5",
    "memcached1.6",
    "redis2.6",
    "redis2.8",
    "redis3.2",
    "redis4.0",
    "redis5.0",
    "redis6.x",
    "redis7",
    "valkey7",
    "valkey8",
];

/// The name prefix of the parameter group AWS itself provides for a family:
/// `default.<family>` (`DeleteCacheParameterGroup`: "You cannot delete the
/// default cache parameter groups in your account.").
pub const DEFAULT_PARAMETER_GROUP_PREFIX: &str = "default.";

/// The published parameter group family `engine_version` of `engine` belongs
/// to, or `None` when no published family covers that version.
///
/// A published family covers a version line, so the version is matched against
/// the lines in [`PARAMETER_GROUP_FAMILIES`] from the most specific to the
/// least: `<major>.<minor>` (Memcached 1.6.22 is `memcached1.6`), then
/// `<major>.x` (Redis OSS 6.2 is `redis6.x`), then `<major>` (Redis OSS 7.1 is
/// `redis7`, Valkey 8.0 is `valkey8`).
///
/// This is also the one statement of which family a version in the published
/// engine-version catalog reports — see [`published_cache_engine_versions`].
pub fn parameter_group_family(engine: &str, engine_version: &str) -> Option<&'static str> {
    let mut parts = engine_version.split('.');
    let major = parts.next().unwrap_or_default();
    let minor = parts.next().unwrap_or("0");
    let candidates = [
        format!("{engine}{major}.{minor}"),
        format!("{engine}{major}.x"),
        format!("{engine}{major}"),
    ];
    candidates.iter().find_map(|candidate| {
        PARAMETER_GROUP_FAMILIES
            .iter()
            .find(|family| **family == candidate.as_str())
            .copied()
    })
}

/// The parameter group a cluster is associated with when the request names
/// none: "the default parameter group for the specified engine"
/// (`CacheParameterGroupName`) — the default group of the version's
/// [`parameter_group_family`], or `None` when no published family covers the
/// version, in which case there is no default group for it to join.
pub fn default_parameter_group_name(engine: &str, engine_version: &str) -> Option<String> {
    parameter_group_family(engine, engine_version)
        .map(|family| format!("{DEFAULT_PARAMETER_GROUP_PREFIX}{family}"))
}

/// The ARN of a cache parameter group:
/// `arn:aws:elasticache:<region>:<account>:parametergroup:<name>`.
pub fn parameter_group_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("parametergroup:{name}"))
}

/// The default parameter groups an account holds the moment ElastiCache becomes
/// available in it: one per published family, which is why
/// `DescribeCacheParameterGroups` answers with groups before anything has been
/// created. Seeded into
/// [`ComAmazonawsElasticacheAmazonelasticachev9State::cache_parameter_groups`]
/// when the store is created, so the groups a read reports and the
/// [`default_parameter_group_name`] a `CreateCacheCluster` applies cannot
/// disagree.
pub fn default_parameter_groups() -> BTreeMap<String, CacheParameterGroupRecord> {
    PARAMETER_GROUP_FAMILIES
        .iter()
        .map(|family| {
            let record = CacheParameterGroupRecord {
                name: format!("{DEFAULT_PARAMETER_GROUP_PREFIX}{family}"),
                family: (*family).to_string(),
                // The description AWS gives a family's own default group.
                description: format!("Default parameter group for {family}"),
            };
            (record.name.clone(), record)
        })
        .collect()
}

/// One cache parameter group, as this emulator persists it.
///
/// Every group this store holds is one AWS provides in EVERY Region (a custom
/// group needs `CreateCacheParameterGroup`, still a scaffolded 501 with its own
/// issue), so the ARN a read reports names the Region of the request reading it
/// — see [`CacheParameterGroupRecord::to_modeled`].
#[derive(Debug, Clone)]
pub struct CacheParameterGroupRecord {
    /// The group's identity, which "is stored as a lowercase string"
    /// (`CreateCacheParameterGroup$CacheParameterGroupName`).
    pub name: String,
    /// One of [`PARAMETER_GROUP_FAMILIES`] (`CacheParameterGroupFamily`).
    pub family: String,
    /// `CacheParameterGroup$Description`.
    pub description: String,
}

impl CacheParameterGroupRecord {
    /// This record as the modelled `CacheParameterGroup`, read from `region`.
    pub fn to_modeled(&self, region: &str) -> CacheParameterGroup {
        CacheParameterGroup {
            cache_parameter_group_name: Some(self.name.clone()),
            cache_parameter_group_family: Some(self.family.clone()),
            description: Some(self.description.clone()),
            // "Indicates whether the parameter group is associated with a
            // Global datastore": a global datastore is a global replication
            // group, and every global-replication-group operation in this
            // service is still a scaffolded 501, so no group is associated with
            // one.
            is_global: Some(false),
            arn: Some(parameter_group_arn(region, &self.name)),
        }
    }
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

/// The ARN of a snapshot: `arn:aws:elasticache:<region>:<account>:snapshot:<name>`.
pub fn snapshot_arn(region: &str, snapshot_name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("snapshot:{snapshot_name}"))
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
/// any integer and the bounds the asking operation's own `MaxRecords` documents
/// are enforced here — `default` and `max` are that operation's, the minimum is
/// the [`MAX_RECORDS_MIN`] every paginated describe in this service shares. It
/// is refused rather than clamped: a caller that asked for a page size the
/// service does not serve gets told so instead of a silently different page.
pub fn page_size_within(max_records: Option<i32>, default: i32, max: i32) -> Result<usize, String> {
    let requested = max_records.unwrap_or(default);
    if !(MAX_RECORDS_MIN..=max).contains(&requested) {
        return Err(format!(
            "Invalid value {requested} for MaxRecords: must be at least {MAX_RECORDS_MIN} and no \
             more than {max}."
        ));
    }
    Ok(usize::try_from(requested).unwrap_or(MAX_RECORDS_MIN as usize))
}

/// The page size `DescribeCacheClusters` / `DescribeCacheSubnetGroups` serve,
/// whose `MaxRecords` documents "Default: 100. Constraints: minimum 20; maximum
/// 100".
pub fn page_size(max_records: Option<i32>) -> Result<usize, String> {
    page_size_within(max_records, MAX_RECORDS_DEFAULT, MAX_RECORDS_MAX)
}

/// The page size `DescribeSnapshots` serves, whose `MaxRecords` documents
/// "Default: 50. Constraints: minimum 20; maximum 50".
pub fn snapshot_page_size(max_records: Option<i32>) -> Result<usize, String> {
    page_size_within(
        max_records,
        SNAPSHOT_MAX_RECORDS_DEFAULT,
        SNAPSHOT_MAX_RECORDS_MAX,
    )
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
    unbounded_page_size_with_default(max_records, MAX_RECORDS_DEFAULT)
}

/// [`unbounded_page_size`], for a describe whose `MaxResults` documents its own
/// default page (`DescribeServerlessCaches`: "The default is 50.").
pub fn unbounded_page_size_with_default(
    max_records: Option<i32>,
    default: i32,
) -> Result<usize, String> {
    let requested = max_records.unwrap_or(default);
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

// ---------------------------------------------------------------------------
// Reserved cache node offerings
//
// An offering is not an account resource: it is an item of the catalog the
// service publishes, which every account sees the same and which no operation
// creates. The catalog below is therefore built once, stocked into the store by
// [`ComAmazonawsElasticacheAmazonelasticachev9State::default`], and read from
// there by `DescribeReservedCacheNodesOfferings` — and by
// `PurchaseReservedCacheNodesOffering` and `DescribeReservedCacheNodes`, which
// filter on the same five members and price a reservation from the same entry.
// ---------------------------------------------------------------------------

/// The reservation terms this service publishes offerings for: the year count
/// `Duration` takes as shorthand, and the term in seconds.
///
/// From `DescribeReservedCacheNodesOfferings$Duration`: "Duration filter value,
/// specified in years or seconds. Valid Values: 1 | 3 | 31536000 | 94608000",
/// so a one-year term is 31536000 seconds and a three-year term 94608000.
pub const OFFERING_TERMS: [(i32, i32); 2] = [(1, 31_536_000), (3, 94_608_000)];

/// The payment options ElastiCache sells reservations under today.
pub const OFFERING_TYPE_ALL_UPFRONT: &str = "All Upfront";
/// See [`OFFERING_TYPE_ALL_UPFRONT`].
pub const OFFERING_TYPE_PARTIAL_UPFRONT: &str = "Partial Upfront";
/// See [`OFFERING_TYPE_ALL_UPFRONT`].
pub const OFFERING_TYPE_NO_UPFRONT: &str = "No Upfront";

/// Every value `OfferingType` documents: "Valid Values: "Light
/// Utilization"|"Medium Utilization"|"Heavy Utilization"|"All Upfront"|"Partial
/// Upfront"|"No Upfront"" — the set a filter value is checked against.
pub const OFFERING_TYPES: [&str; 6] = [
    "Light Utilization",
    "Medium Utilization",
    "Heavy Utilization",
    OFFERING_TYPE_ALL_UPFRONT,
    OFFERING_TYPE_PARTIAL_UPFRONT,
    OFFERING_TYPE_NO_UPFRONT,
];

/// The offering types the catalog is STOCKED with: the three payment options
/// above. The three legacy utilization types are documented filter values all
/// the same, so a request naming one is served rather than refused, and matches
/// no offering — exactly as it does against a region that sells none.
pub const STOCKED_OFFERING_TYPES: [&str; 3] = [
    OFFERING_TYPE_ALL_UPFRONT,
    OFFERING_TYPE_PARTIAL_UPFRONT,
    OFFERING_TYPE_NO_UPFRONT,
];

/// The `RecurringChargeFrequency` of a reservation's recurring charge: the
/// hourly charge a partial- or no-upfront reservation is billed at.
pub const RECURRING_CHARGE_HOURLY: &str = "Hourly";

/// Seconds in an hour, which is what turns a term into the hours a fixed price
/// covers.
const SECONDS_PER_HOUR: f64 = 3_600.0;

/// The share of a partial-upfront reservation's cost that is paid up front; the
/// rest is the recurring hourly charge.
const PARTIAL_UPFRONT_SHARE: f64 = 0.5;

/// The cache node types the offering catalog covers, each with the hourly
/// on-demand rate its reservations are priced from.
///
/// The node types are from `ReservedCacheNodesOffering$CacheNodeType`'s own list
/// (`smithy_query(query="doc", shape="ReservedCacheNodesOffering",
/// member="CacheNodeType")`), spanning the current-generation general purpose
/// (T4g/T3, M7g/M6g/M5) and memory optimized (R7g/R6g/R5) families and the
/// previous generation (M3, R3, T2) that list also names.
///
/// The rates are the EMULATOR's own, not a claim about AWS's prices: AWS
/// publishes its price list outside this API and no ElastiCache operation
/// reports it. They are a deterministic scale — a family rate times the node
/// size — so the prices this catalog reports are consistent between node types
/// and stable across reads.
pub const OFFERING_NODE_TYPES: [(&str, f64); 19] = [
    ("cache.m3.medium", 0.08),
    ("cache.m5.2xlarge", 0.64),
    ("cache.m5.large", 0.16),
    ("cache.m5.xlarge", 0.32),
    ("cache.m6g.large", 0.16),
    ("cache.m6g.xlarge", 0.32),
    ("cache.m7g.large", 0.16),
    ("cache.m7g.xlarge", 0.32),
    ("cache.r3.large", 0.24),
    ("cache.r5.2xlarge", 0.96),
    ("cache.r5.large", 0.24),
    ("cache.r5.xlarge", 0.48),
    ("cache.r6g.large", 0.24),
    ("cache.r7g.large", 0.24),
    ("cache.t2.micro", 0.02),
    ("cache.t3.medium", 0.08),
    ("cache.t3.micro", 0.02),
    ("cache.t3.small", 0.04),
    ("cache.t4g.micro", 0.02),
];

/// `value` rounded to `places` decimal places, so a reported price is a money
/// amount rather than the full binary expansion of the product behind it.
fn round_to(value: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    (value * scale).round() / scale
}

/// The share of the on-demand rate a reservation's effective hourly rate is,
/// per payment option and per TERM POSITION in [`OFFERING_TERMS`].
///
/// The EMULATOR's own scale (see [`OFFERING_NODE_TYPES`]), fixed so that it
/// keeps the ordering a reservation is bought for: a longer term discounts more
/// than a shorter one, and more money up front discounts more than less.
/// Indexing by the term's position means a term added to `OFFERING_TERMS` has
/// to be priced here too, instead of silently taking the shortest term's share.
const OFFERING_RATE_SHARES: [(&str, [f64; OFFERING_TERMS.len()]); STOCKED_OFFERING_TYPES.len()] = [
    (OFFERING_TYPE_ALL_UPFRONT, [0.60, 0.45]),
    (OFFERING_TYPE_PARTIAL_UPFRONT, [0.65, 0.50]),
    (OFFERING_TYPE_NO_UPFRONT, [0.75, 0.60]),
];

/// The entry of [`OFFERING_RATE_SHARES`] this payment option and term are
/// priced by.
///
/// Both lookups are over the catalog's own constants — the only caller builds
/// the catalog out of [`OFFERING_TERMS`] and [`STOCKED_OFFERING_TYPES`] — so a
/// miss would mean the pricing table lost a row the catalog still stocks, which
/// is a bug in this module rather than anything a request can ask for.
fn offering_rate_share(offering_type: &str, duration_seconds: i32) -> f64 {
    let term = OFFERING_TERMS
        .iter()
        .position(|(_, seconds)| *seconds == duration_seconds)
        .expect("a stocked offering's term is one of OFFERING_TERMS");
    OFFERING_RATE_SHARES
        .iter()
        .find(|(option, _)| *option == offering_type)
        .map(|(_, shares)| shares[term])
        .expect("a stocked offering's payment option is priced by OFFERING_RATE_SHARES")
}

/// The `ReservedCacheNodesOfferingId` of the offering with these attributes.
///
/// "A unique identifier for the reserved cache node offering", in the form the
/// member's own example gives (`438012d3-4052-4cc7-b2e3-8d3372e0e706`): five
/// hyphen-separated hex groups. It is DERIVED from the attributes that make the
/// offering, so it is unique per offering and identical on every read, and the
/// catalog needs no counter to mint it.
pub fn offering_id(
    product_description: &str,
    cache_node_type: &str,
    duration_seconds: i32,
    offering_type: &str,
) -> String {
    use ::sha2::Digest;
    let mut hasher = ::sha2::Sha256::new();
    hasher.update(
        format!(
            "{ARN_NAMESPACE}|{product_description}|{cache_node_type}|{duration_seconds}|{offering_type}"
        )
        .as_bytes(),
    );
    let digest = ::hex::encode(hasher.finalize());
    format!(
        "{}-{}-{}-{}-{}",
        &digest[0..8],
        &digest[8..12],
        &digest[12..16],
        &digest[16..20],
        &digest[20..32]
    )
}

/// The key an offering is held under, which is also the total order
/// `DescribeReservedCacheNodesOfferings` pages through and the position its
/// `Marker` resumes from.
///
/// The term is zero-padded so the key orders by term rather than by the digits
/// of the second count.
fn offering_key(
    product_description: &str,
    cache_node_type: &str,
    duration_seconds: i32,
    offering_type: &str,
) -> String {
    format!("{product_description}|{cache_node_type}|{duration_seconds:010}|{offering_type}")
}

/// One catalog entry, priced from the node type's hourly rate.
///
/// `FixedPrice` is "the fixed price charged for this offering" — the whole term
/// for an all-upfront reservation, half of it for a partial-upfront one, nothing
/// for a no-upfront one — and whatever is not paid up front is the hourly
/// `RecurringCharges` entry. `UsagePrice`, "the hourly price charged for this
/// offering", is 0 for all three: an upfront-type reservation's hourly charge is
/// the recurring charge, not a usage price.
fn stock_offering(
    product_description: &str,
    cache_node_type: &str,
    base_hourly: f64,
    duration_seconds: i32,
    offering_type: &'static str,
) -> ReservedCacheNodesOffering {
    let hours = f64::from(duration_seconds) / SECONDS_PER_HOUR;
    let effective_hourly = base_hourly * offering_rate_share(offering_type, duration_seconds);
    let (fixed_price, recurring_hourly) = if offering_type == OFFERING_TYPE_ALL_UPFRONT {
        (effective_hourly * hours, 0.0)
    } else if offering_type == OFFERING_TYPE_PARTIAL_UPFRONT {
        (
            effective_hourly * hours * PARTIAL_UPFRONT_SHARE,
            effective_hourly * (1.0 - PARTIAL_UPFRONT_SHARE),
        )
    } else {
        (0.0, effective_hourly)
    };
    let recurring_charges = if recurring_hourly > 0.0 {
        vec![RecurringCharge {
            recurring_charge_amount: Some(round_to(recurring_hourly, 4)),
            recurring_charge_frequency: Some(RECURRING_CHARGE_HOURLY.to_string()),
        }]
    } else {
        Vec::new()
    };
    ReservedCacheNodesOffering {
        reserved_cache_nodes_offering_id: Some(offering_id(
            product_description,
            cache_node_type,
            duration_seconds,
            offering_type,
        )),
        cache_node_type: Some(cache_node_type.to_string()),
        duration: Some(duration_seconds),
        fixed_price: Some(round_to(fixed_price, 2)),
        usage_price: Some(0.0),
        product_description: Some(product_description.to_string()),
        offering_type: Some(offering_type.to_string()),
        recurring_charges: Some(recurring_charges),
    }
}

/// The whole published catalog: one offering per cache engine, node type, term
/// and stocked payment option, keyed by [`offering_key`].
///
/// A `BTreeMap` is what gives the describe the stable total order its `Marker`
/// indexes into, the same way the cluster and subnet-group maps do.
pub fn offering_catalog() -> BTreeMap<String, ReservedCacheNodesOffering> {
    let mut catalog = BTreeMap::new();
    // `ProductDescription` is "the cache engine used by the offering", so the
    // catalog carries one offering per engine this service runs.
    for product_description in ENGINES {
        for (cache_node_type, base_hourly) in OFFERING_NODE_TYPES {
            for (_, duration_seconds) in OFFERING_TERMS {
                for offering_type in STOCKED_OFFERING_TYPES {
                    catalog.insert(
                        offering_key(
                            product_description,
                            cache_node_type,
                            duration_seconds,
                            offering_type,
                        ),
                        stock_offering(
                            product_description,
                            cache_node_type,
                            base_hourly,
                            duration_seconds,
                            offering_type,
                        ),
                    );
                }
            }
        }
    }
    catalog
}

/// The term in SECONDS a `Duration` filter value names, or the reason it names
/// none.
///
/// `Duration` is a plain string with no modelled constraint, so the documented
/// "Valid Values: 1 | 3 | 31536000 | 94608000" is enforced here; a year count
/// and the matching second count select the same offerings.
pub fn parse_offering_duration(value: &str) -> Result<i32, String> {
    OFFERING_TERMS
        .iter()
        .find(|(years, seconds)| value == years.to_string() || value == seconds.to_string())
        .map(|(_, seconds)| *seconds)
        .ok_or_else(|| {
            let valid = OFFERING_TERMS
                .iter()
                .flat_map(|(years, seconds)| [years.to_string(), seconds.to_string()])
                .collect::<Vec<_>>()
                .join(" | ");
            format!("Invalid value {value} for Duration: must be one of {valid}.")
        })
}

/// The documented `OfferingType` `value` names, or the reason it names none.
///
/// Matched ignoring ASCII case so the answer does not turn on how the caller
/// capitalised a documented value, and the canonical spelling is what any
/// comparison then uses.
pub fn canonical_offering_type(value: &str) -> Result<&'static str, String> {
    OFFERING_TYPES
        .into_iter()
        .find(|candidate| candidate.eq_ignore_ascii_case(value))
        .ok_or_else(|| {
            format!(
                "Invalid value {value} for OfferingType: must be one of {}.",
                OFFERING_TYPES.join(" | ")
            )
        })
}

/// The five filter members `DescribeReservedCacheNodesOfferings` narrows the
/// catalog with (and `DescribeReservedCacheNodes` documents the same five for
/// the reservations bought from it).
///
/// A member the request omits filters nothing; one it sets keeps only the
/// offerings whose value matches it, so several set together intersect.
#[derive(Debug, Default, Clone)]
pub struct OfferingFilter {
    /// `ReservedCacheNodesOfferingId` — "show only the available offering that
    /// matches the specified reservation identifier". Matched exactly: it is an
    /// opaque identifier.
    pub offering_id: Option<String>,
    /// `CacheNodeType` — "show only the available offerings matching the
    /// specified cache node type".
    pub cache_node_type: Option<String>,
    /// `Duration`, as the term in seconds that
    /// [`parse_offering_duration`] resolved it to.
    pub duration_seconds: Option<i32>,
    /// `ProductDescription` — "show only the available offerings matching the
    /// specified product description".
    pub product_description: Option<String>,
    /// `OfferingType`, in the canonical spelling
    /// [`canonical_offering_type`] resolved.
    pub offering_type: Option<&'static str>,
}

impl OfferingFilter {
    /// True when a record carrying these five attributes satisfies every member
    /// this filter sets.
    ///
    /// The offering id is compared EXACTLY — it is an opaque identifier, so one
    /// that differs anywhere, case included, names a different offering, which
    /// is what lets the caller be told it does not exist rather than handed an
    /// empty page. The node type, the engine and the offering type are compared
    /// ignoring ASCII case: a node type and an engine name are both stored
    /// lowercase, and an offering type is a documented display spelling, so case
    /// is part of none of their identities.
    ///
    /// Taking the attributes rather than a shape is what lets a published
    /// offering and a reservation bought from one be narrowed by ONE
    /// implementation of the five comparisons, so the two describes cannot
    /// disagree about what a filter value selects.
    fn matches_attributes(
        &self,
        offering_id: Option<&str>,
        cache_node_type: Option<&str>,
        duration_seconds: Option<i32>,
        product_description: Option<&str>,
        offering_type: Option<&str>,
    ) -> bool {
        let same = |filter: Option<&str>, value: Option<&str>| {
            filter.is_none_or(|wanted| {
                value.is_some_and(|actual| actual.eq_ignore_ascii_case(wanted))
            })
        };
        self.offering_id
            .as_deref()
            .is_none_or(|wanted| offering_id.is_some_and(|actual| actual == wanted))
            && same(self.cache_node_type.as_deref(), cache_node_type)
            && same(self.product_description.as_deref(), product_description)
            && self
                .duration_seconds
                .is_none_or(|wanted| duration_seconds == Some(wanted))
            && same(self.offering_type, offering_type)
    }

    /// True when `offering` satisfies every member this filter sets.
    pub fn matches(&self, offering: &ReservedCacheNodesOffering) -> bool {
        self.matches_attributes(
            offering.reserved_cache_nodes_offering_id.as_deref(),
            offering.cache_node_type.as_deref(),
            offering.duration,
            offering.product_description.as_deref(),
            offering.offering_type.as_deref(),
        )
    }

    /// True when `reservation` satisfies every member this filter sets.
    ///
    /// `DescribeReservedCacheNodes` documents the same filter members over the
    /// reservations bought from the catalog as
    /// `DescribeReservedCacheNodesOfferings` documents over the catalog itself,
    /// so the comparison is the same one, made against the reservation's own
    /// copy of the offering's attributes.
    pub fn matches_reservation(&self, reservation: &ReservedCacheNodeRecord) -> bool {
        self.matches_attributes(
            Some(&reservation.reserved_cache_nodes_offering_id),
            reservation.cache_node_type.as_deref(),
            reservation.duration_seconds,
            reservation.product_description.as_deref(),
            reservation.offering_type.as_deref(),
        )
    }
}

// ---------------------------------------------------------------------------
// Reserved cache nodes
//
// A reserved cache node — a reservation — IS an account resource, unlike the
// offering it is bought from: `PurchaseReservedCacheNodesOffering` mints one
// from a catalog entry above and `DescribeReservedCacheNodes` reports it. The
// record below is the one copy both read, so the two cannot disagree about
// what was bought.
// ---------------------------------------------------------------------------

/// The number of nodes a purchase reserves when the request names none, as
/// `PurchaseReservedCacheNodesOffering$CacheNodeCount` documents it: "The
/// number of cache node instances to reserve. Default: 1".
pub const DEFAULT_CACHE_NODE_COUNT: i32 = 1;

/// The states a reservation is reported in.
///
/// `ReservedCacheNode$State` is documented only as "the state of the reserved
/// cache node" and the model declares no enum for it, so these are the states
/// ElastiCache reports a reservation in: a purchase is `payment-pending` until
/// its payment is taken, `active` for the term it was bought for, and `retired`
/// once that term has elapsed.
pub const RESERVATION_STATE_PAYMENT_PENDING: &str = "payment-pending";
/// See [`RESERVATION_STATE_PAYMENT_PENDING`].
pub const RESERVATION_STATE_ACTIVE: &str = "active";
/// See [`RESERVATION_STATE_PAYMENT_PENDING`].
pub const RESERVATION_STATE_RETIRED: &str = "retired";

/// How long a reservation stays `payment-pending` before the clock owes it
/// `active`.
///
/// The same reasoning as [`CLUSTER_TRANSITION`]: AWS takes as long as the
/// payment takes, the emulator chooses a span, and it is deliberately NOT zero
/// so a caller describing straight after the purchase still observes the
/// transient state. The due instant is stored on the record and materialised by
/// whichever later call reads it — nothing sleeps.
pub const RESERVATION_TRANSITION: Duration = Duration::from_millis(500);

/// How many reserved cache nodes the store holds before a purchase is refused
/// with the modelled `ReservedCacheNodeQuotaExceededFault` — "the request
/// cannot be processed because it would exceed the user's cache node quota".
///
/// See [`CLUSTER_QUOTA`]: the emulator's OWN bound on the store rather than a
/// claim about any account's limit, set at the same count as the cluster quota
/// because both bound cache nodes.
pub const RESERVED_CACHE_NODE_QUOTA: i32 = 300;

/// The ARN of a reserved cache node, in the form
/// `ReservedCacheNode$ReservationARN`'s own example gives
/// (`arn:aws:elasticache:us-east-1:123456789012:reserved-instance:ri-2017-03-27-08-33-25-582`).
pub fn reservation_arn(region: &str, reserved_cache_node_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("reserved-instance:{reserved_cache_node_id}"),
    )
}

/// The identifier a reservation the request named none carries:
/// `PurchaseReservedCacheNodesOffering$ReservedCacheNodeId` documents "if this
/// parameter is not specified, ElastiCache automatically generates an
/// identifier for the reservation".
///
/// The generated form is the one `ReservationARN`'s example carries — `ri-`
/// followed by the purchase instant with every field hyphen-separated
/// (`ri-2017-03-27-08-33-25-582`) — built from the RFC-3339 rendering of the
/// instant, so the service's one timestamp implementation is also what names a
/// reservation.
pub fn generated_reservation_id(at: SystemTime) -> String {
    let rendered = ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default();
    let stamp: String = rendered
        .trim_end_matches('Z')
        .chars()
        .map(|c| if c.is_ascii_digit() { c } else { '-' })
        .collect();
    format!("ri-{stamp}")
}

/// The values a new reservation carries that do not come from the offering it
/// is bought from — one argument so
/// [`ReservedCacheNodeRecord::from_offering`] does not take a row of positional
/// values.
#[derive(Debug, Clone)]
pub struct NewReservation {
    /// The reservation's identity: the `ReservedCacheNodeId` the request named,
    /// or the one [`generated_reservation_id`] minted for it.
    pub reserved_cache_node_id: String,
    /// `CacheNodeCount` — "the number of cache node instances to reserve".
    pub cache_node_count: i32,
    /// The Region of the request that bought it, which its ARN names.
    pub region: String,
    /// The tags the purchase was given. `AddTagsToResource` /
    /// `ListTagsForResource` are scaffolded 501s and own their issues; the tags
    /// are persisted here so implementing them does not have to move them.
    pub tags: Vec<(String, String)>,
    pub now: SystemTime,
}

/// One reserved cache node, as this emulator persists it.
///
/// Every priced member is COPIED out of the catalog entry the reservation was
/// bought from, which is what makes a reservation a record of what was bought
/// rather than a second view of the live catalog.
#[derive(Debug, Clone)]
pub struct ReservedCacheNodeRecord {
    /// "The unique identifier for the reservation", which is this record's
    /// identity in the store.
    pub reserved_cache_node_id: String,
    /// The offering it was bought from.
    pub reserved_cache_nodes_offering_id: String,
    pub cache_node_type: Option<String>,
    /// "The time the reservation started" — the instant of the purchase.
    pub start_time: SystemTime,
    /// "The duration of the reservation in seconds", from the offering's term.
    pub duration_seconds: Option<i32>,
    pub fixed_price: Option<f64>,
    pub usage_price: Option<f64>,
    /// "The number of cache nodes that have been reserved."
    pub cache_node_count: i32,
    pub product_description: Option<String>,
    pub offering_type: Option<String>,
    /// One of [`RESERVATION_STATE_PAYMENT_PENDING`],
    /// [`RESERVATION_STATE_ACTIVE`] or [`RESERVATION_STATE_RETIRED`].
    pub state: String,
    /// The state change the clock owes this reservation, if any.
    pub pending_state: Option<PendingTransition>,
    /// "The recurring price charged to run this reserved cache node", from the
    /// offering.
    pub recurring_charges: Vec<RecurringCharge>,
    pub arn: String,
    /// See [`NewReservation::tags`].
    pub tags: Vec<(String, String)>,
}

impl ReservedCacheNodeRecord {
    /// The reservation `new` buys from `offering`.
    pub fn from_offering(offering: &ReservedCacheNodesOffering, new: NewReservation) -> Self {
        Self {
            arn: reservation_arn(&new.region, &new.reserved_cache_node_id),
            reserved_cache_node_id: new.reserved_cache_node_id,
            reserved_cache_nodes_offering_id: offering
                .reserved_cache_nodes_offering_id
                .clone()
                .unwrap_or_default(),
            cache_node_type: offering.cache_node_type.clone(),
            start_time: new.now,
            duration_seconds: offering.duration,
            // The offering's prices are per reserved cache node — "the fixed
            // price charged for this offering" and "the hourly price charged
            // for this offering" — so a multi-node reservation reports the same
            // per-node prices beside its `CacheNodeCount` rather than a total
            // no member of this shape documents.
            fixed_price: offering.fixed_price,
            usage_price: offering.usage_price,
            cache_node_count: new.cache_node_count,
            product_description: offering.product_description.clone(),
            offering_type: offering.offering_type.clone(),
            state: RESERVATION_STATE_PAYMENT_PENDING.to_string(),
            pending_state: Some(PendingTransition {
                due: new.now + RESERVATION_TRANSITION,
                status: RESERVATION_STATE_ACTIVE.to_string(),
            }),
            recurring_charges: offering.recurring_charges.clone().unwrap_or_default(),
            tags: new.tags,
        }
    }

    /// Materialise every state change the clock owes this reservation. The one
    /// write a reporting operation may make; see [`SnapshotRecord::settle`].
    pub fn settle(&mut self, now: SystemTime) {
        self.settle_payment(now);
        self.settle_term(now);
    }

    /// `payment-pending` becomes `active` once the payment's due instant has
    /// passed.
    fn settle_payment(&mut self, now: SystemTime) {
        let Some(pending) = self.pending_state.as_ref() else {
            return;
        };
        if now < pending.due {
            return;
        }
        self.state = pending.status.clone();
        self.pending_state = None;
    }

    /// An `active` reservation becomes `retired` once the term it was bought
    /// for has elapsed: the reservation runs for its offering's `Duration`
    /// ("the duration of the reservation in seconds") from its `StartTime`, and
    /// that span is the offering's own rather than one the emulator chose.
    fn settle_term(&mut self, now: SystemTime) {
        if self.state != RESERVATION_STATE_ACTIVE {
            return;
        }
        let Some(seconds) = self
            .duration_seconds
            .and_then(|seconds| u64::try_from(seconds).ok())
        else {
            return;
        };
        if now < self.start_time + Duration::from_secs(seconds) {
            return;
        }
        self.state = RESERVATION_STATE_RETIRED.to_string();
    }

    /// This record as the modelled `ReservedCacheNode`.
    pub fn to_modeled(&self) -> ReservedCacheNode {
        ReservedCacheNode {
            reserved_cache_node_id: Some(self.reserved_cache_node_id.clone()),
            reserved_cache_nodes_offering_id: Some(self.reserved_cache_nodes_offering_id.clone()),
            cache_node_type: self.cache_node_type.clone(),
            start_time: Some(::aws_smithy_types::DateTime::from(self.start_time)),
            duration: self.duration_seconds,
            fixed_price: self.fixed_price,
            usage_price: self.usage_price,
            cache_node_count: Some(self.cache_node_count),
            product_description: self.product_description.clone(),
            offering_type: self.offering_type.clone(),
            state: Some(self.state.clone()),
            recurring_charges: Some(self.recurring_charges.clone()),
            reservation_arn: Some(self.arn.clone()),
        }
    }
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
            // The primary a read replica syncs from, as a cache NODE id within
            // the reporting cluster. A standalone cluster has no primary, and a
            // replication group's member cluster holds a single node whose
            // primary is a node of a DIFFERENT cluster, which this member cannot
            // name — the group's `NodeGroupMembers`/`CurrentRole` is where that
            // relationship is reported.
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
    /// The replication group this cluster belongs to: set on the member
    /// clusters `CreateReplicationGroup` mints, absent on a standalone cluster.
    /// The field is what
    /// `DescribeCacheClusters$ShowCacheClustersNotInReplicationGroups` filters
    /// on, so that filter reads stored state rather than a constant.
    pub replication_group_id: Option<String>,
    /// `CacheCluster$ReplicationGroupLogDeliveryEnabled` — whether log delivery
    /// is enabled for the replication group this cluster belongs to. `None` on a
    /// standalone cluster, which belongs to no group.
    pub replication_group_log_delivery_enabled: Option<bool>,
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
    /// The in-transit encryption MODE, which only a replication group sets, so
    /// it is absent on a standalone cluster.
    pub transit_encryption_mode: Option<String>,
    /// Whether data is encrypted at rest. `CreateCacheCluster` has no member for
    /// it, so it is absent on a standalone cluster and carries the group's
    /// setting on a member cluster.
    pub at_rest_encryption_enabled: Option<bool>,
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
    ///
    /// `target` is clamped to a count some engine permits (1 ..=
    /// [`MEMCACHED_MAX_NODES`]) before anything is allocated. Both callers
    /// already refuse a count outside what their engine permits
    /// ([`validate_num_cache_nodes`]) so the clamp changes no request the service
    /// accepts; it is what keeps ONE request from sizing the allocation this
    /// loop does, and it is the value stored, so the node list and
    /// `num_cache_nodes` cannot disagree.
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
        // The growth loop below allocates one record per node, so the node
        // count is bounded HERE — where both the create and the modify path go
        // through it — rather than left to each caller's own validation. No
        // engine permits more than [`MEMCACHED_MAX_NODES`] nodes, and no
        // cluster has fewer than one (see [`validate_num_cache_nodes`]), so a
        // count outside that range cannot be a cluster this service serves and
        // is clamped instead of allocated.
        let target = target.clamp(1, MEMCACHED_MAX_NODES);
        let target_len = usize::try_from(target).unwrap_or(1);
        self.nodes
            .reserve(target_len.saturating_sub(self.nodes.len()));
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
            // `saturating_add`: the ordinal is bounded by the clamp above, so
            // this cannot wrap, and it must not panic in a debug build if a
            // future caller ever brings it close.
            highest = highest.saturating_add(1);
            let zone = if new_zones.is_empty() {
                self.zone_for(usize::try_from(highest).unwrap_or(1).saturating_sub(1))
            } else {
                Some(new_zones[added % new_zones.len()].clone())
            };
            let node = self.new_node(highest, now, status, zone);
            self.nodes.push(node);
            added += 1;
        }
        // The count stored is the clamped target, which is also the number of
        // nodes the loop above left in place, so the two cannot disagree.
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
            // The group this cluster is a member of, set when
            // `CreateReplicationGroup` minted it as one of the group's member
            // clusters.
            replication_group_id: self.replication_group_id.clone(),
            snapshot_retention_limit: self.snapshot_retention_limit,
            snapshot_window: self.snapshot_window.clone(),
            auth_token_enabled: Some(self.auth_token_enabled),
            auth_token_last_modified_date: self
                .auth_token_last_modified
                .map(::aws_smithy_types::DateTime::from),
            transit_encryption_enabled: self.transit_encryption_enabled,
            // At-rest encryption "cannot be modified after the cluster is
            // created" and `CreateCacheCluster` has no member to set it, so a
            // standalone cluster carries none; a replication group's member
            // cluster carries the group's setting.
            at_rest_encryption_enabled: self.at_rest_encryption_enabled,
            arn: Some(self.arn.clone()),
            // Log delivery "for the replication group": absent on a standalone
            // cluster, and on a member cluster whatever its group was created
            // with.
            replication_group_log_delivery_enabled: self.replication_group_log_delivery_enabled,
            log_delivery_configurations: (!self.log_delivery_configurations.is_empty()).then(|| {
                self.log_delivery_configurations
                    .iter()
                    .map(LogDeliveryRecord::to_modeled)
                    .collect()
            }),
            network_type: self.network_type.clone(),
            ip_discovery: self.ip_discovery.clone(),
            // The in-transit encryption MODE is set when a replication group is
            // created; `CreateCacheCluster` has no member for it, so a
            // standalone cluster carries none.
            transit_encryption_mode: self.transit_encryption_mode.clone(),
        }
    }
}

/// The cache engines a replication group may run, from
/// `CreateReplicationGroup$Engine`: "The value must be set to valkey or redis",
/// and the operation itself "is valid for Valkey and Redis OSS only".
pub const REPLICATION_GROUP_ENGINES: [&str; 2] = [DEFAULT_ENGINE, ENGINE_VALKEY];

/// The largest `NumCacheClusters`, as the member documents it: "The maximum
/// permitted value for NumCacheClusters is 6 (1 primary plus 5 replicas)."
pub const MAX_CACHE_CLUSTERS_PER_GROUP: i32 = 6;

/// The largest `ReplicasPerNodeGroup` / `NodeGroupConfiguration$ReplicaCount`,
/// as `ReplicasPerNodeGroup` documents it: "Valid values are 0 to 5."
pub const MAX_REPLICAS_PER_NODE_GROUP: i32 = 5;

/// The largest `NumNodeGroups`, from
/// `NodeGroupsPerReplicationGroupQuotaExceededFault`: "The request cannot be
/// processed because it would exceed the maximum allowed number of node groups
/// (shards) in a single replication group. The default maximum is 90".
pub const MAX_NODE_GROUPS: i32 = 90;

/// How many hash slots a Valkey / Redis OSS keyspace holds, from
/// `NodeGroupConfiguration$Slots`: "Keyspaces range from 0 to 16,383."
pub const KEYSPACE_SLOTS: i32 = 16_384;

/// `AuthToken`'s documented length bounds: "Must be at least 16 characters and
/// no more than 128 characters in length."
pub const AUTH_TOKEN_MIN_LEN: usize = 16;
/// See [`AUTH_TOKEN_MIN_LEN`].
pub const AUTH_TOKEN_MAX_LEN: usize = 128;

/// The printable special characters an `AuthToken` may carry: "The only
/// permitted printable special characters are !, &, #, $, ^, <, >, and -. Other
/// printable special characters cannot be used in the AUTH token."
pub const AUTH_TOKEN_SPECIAL_CHARACTERS: [char; 8] = ['!', '&', '#', '$', '^', '<', '>', '-'];

/// The node-type family data tiering requires, from `DataTieringEnabled`: "Data
/// tiering is only supported for replication groups using the r6gd node type.
/// This parameter must be set to true when using r6gd nodes."
pub const DATA_TIERING_NODE_TYPE: &str = "r6gd";

/// The `enabled` / `disabled` of `AutomaticFailoverStatus`, `MultiAZStatus` and
/// `DataTieringStatus` (`smithy_query(query="shape", shape="MultiAZStatus")`).
pub const SETTING_ENABLED: &str = "enabled";
/// See [`SETTING_ENABLED`].
pub const SETTING_DISABLED: &str = "disabled";

/// `NodeGroupMember$CurrentRole`'s values: "The role that is currently assigned
/// to the node - primary or replica."
pub const ROLE_PRIMARY: &str = "primary";
/// See [`ROLE_PRIMARY`].
pub const ROLE_REPLICA: &str = "replica";

/// `StorageEncryptionType`'s values (`smithy_query(query="shape",
/// shape="StorageEncryptionType")`: `none | sse-elasticache | sse-kms`), which
/// `ReplicationGroup$StorageEncryptionType` documents as "none if at-rest
/// encryption is not enabled, sse-elasticache if an ElastiCache service-managed
/// key is used, or sse-kms if a customer-managed KMS key is used". The two
/// key-bearing values are declared with the serverless-cache slice, which
/// documents them the same way.
pub const STORAGE_ENCRYPTION_NONE: &str = "none";

/// `Durability`'s `default`: "When set to default, the service determines the
/// effective durability based on the engine version, cluster mode, and other
/// parameters. The resolved setting is reflected in the EffectiveDurability
/// property."
pub const DURABILITY_DEFAULT: &str = "default";

/// What [`DURABILITY_DEFAULT`] resolves to here, and the only value
/// `EffectiveDurability` can take for a group this operation creates:
/// `CreateReplicationGroup`'s own documentation states the replication this
/// service performs — "Writes to the primary are asynchronously propagated to
/// the replicas" — and `async` is that setting's name in `EffectiveDurability`
/// (`async | sync | disabled`).
pub const EFFECTIVE_DURABILITY_ASYNC: &str = "async";

/// The id of the one cache node a Valkey / Redis OSS member cluster holds; node
/// ids are "a numeric identifier (0001, 0002, etc.)" and
/// [`REDIS_NUM_CACHE_NODES`] is 1, so a member cluster's node is always this one.
pub const FIRST_CACHE_NODE_ID: &str = "0001";

/// The ARN of a replication group:
/// `arn:aws:elasticache:<region>:<account>:replicationgroup:<id>`.
pub fn replication_group_arn(region: &str, replication_group_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("replicationgroup:{replication_group_id}"),
    )
}

/// `engine` lowercased when it names an engine a replication group may run,
/// else the reason it is refused. See [`REPLICATION_GROUP_ENGINES`].
pub fn normalize_replication_group_engine(engine: &str) -> Result<String, String> {
    let lowered = engine.to_ascii_lowercase();
    if REPLICATION_GROUP_ENGINES.contains(&lowered.as_str()) {
        Ok(lowered)
    } else {
        Err(format!(
            "Invalid value {engine} for Engine: a replication group must run one of {}.",
            REPLICATION_GROUP_ENGINES.join(" | ")
        ))
    }
}

/// `None` when `token` satisfies every documented `AuthToken` constraint, else
/// the reason it does not: "Must be only printable ASCII characters. Must be at
/// least 16 characters and no more than 128 characters in length. The only
/// permitted printable special characters are !, &, #, $, ^, <, >, and -."
pub fn auth_token_refusal(token: &str) -> Option<String> {
    if !(AUTH_TOKEN_MIN_LEN..=AUTH_TOKEN_MAX_LEN).contains(&token.chars().count()) {
        return Some(format!(
            "Invalid value for AuthToken: the password must be at least {AUTH_TOKEN_MIN_LEN} and \
             no more than {AUTH_TOKEN_MAX_LEN} characters in length."
        ));
    }
    if !token.chars().all(|character| {
        character.is_ascii_alphanumeric() || AUTH_TOKEN_SPECIAL_CHARACTERS.contains(&character)
    }) {
        return Some(format!(
            "Invalid value for AuthToken: the password must be printable ASCII characters, and \
             the only permitted printable special characters are {}.",
            AUTH_TOKEN_SPECIAL_CHARACTERS
                .iter()
                .map(char::to_string)
                .collect::<Vec<String>>()
                .join(", ")
        ));
    }
    None
}

/// The inclusive slot range a `Slots` string names, or the reason it names none.
///
/// "A string that specifies the keyspace for a particular node group. Keyspaces
/// range from 0 to 16,383. The string is in the format `startkey-endkey`.
/// Example: `"0-3999"`."
pub fn slot_range(slots: &str) -> Result<(i32, i32), String> {
    let refusal = || {
        format!(
            "Invalid value {slots} for Slots: a keyspace is written startkey-endkey, with both \
             keys between 0 and {}.",
            KEYSPACE_SLOTS - 1
        )
    };
    let (start, end) = slots.split_once('-').ok_or_else(refusal)?;
    let start: i32 = start.trim().parse().map_err(|_| refusal())?;
    let end: i32 = end.trim().parse().map_err(|_| refusal())?;
    if start < 0 || end < start || end >= KEYSPACE_SLOTS {
        return Err(refusal());
    }
    Ok((start, end))
}

/// The keyspace of shard `index` of `num_node_groups` when the request named
/// none: the 16,384 slots split as evenly as they divide, in order, the first
/// shards taking the remainder, so the shards together cover the whole keyspace
/// exactly once.
pub fn default_slots(num_node_groups: i32, index: i32) -> String {
    let total = i64::from(KEYSPACE_SLOTS);
    let shards = i64::from(num_node_groups.max(1));
    let index = i64::from(index);
    let size = total / shards;
    let remainder = total % shards;
    // Each shard before this one took `size` slots, plus one more while the
    // remainder lasted.
    let start = index * size + index.min(remainder);
    let end = start + size + i64::from(index < remainder) - 1;
    format!("{start}-{end}")
}

/// The id ElastiCache gives the `member_ordinal`-th member cluster (1-based) of
/// the node group (shard) `node_group_id` names.
///
/// A Valkey or Redis OSS (cluster mode disabled) group numbers its member
/// clusters `<group>-001`, `<group>-002`, …; a cluster mode enabled group names
/// the shard too, `<group>-0001-001`. Both are derived from the group id and the
/// shard's own id, so a member cluster is always resolvable from the group and
/// the shard a caller knows.
pub fn member_cluster_id(
    replication_group_id: &str,
    node_group_id: &str,
    member_ordinal: usize,
    cluster_enabled: bool,
) -> String {
    if cluster_enabled {
        format!("{replication_group_id}-{node_group_id}-{member_ordinal:03}")
    } else {
        format!("{replication_group_id}-{member_ordinal:03}")
    }
}

/// One member of a node group (shard), as this emulator persists it: which
/// cluster and node it is, where it sits and which role it plays.
#[derive(Debug, Clone)]
pub struct NodeGroupMemberRecord {
    pub cache_cluster_id: String,
    pub cache_node_id: String,
    pub availability_zone: Option<String>,
    /// [`ROLE_PRIMARY`] or [`ROLE_REPLICA`].
    pub current_role: String,
}

impl NodeGroupMemberRecord {
    /// This member as the modelled `NodeGroupMember`.
    ///
    /// `ReadEndpoint` and `CurrentRole` are reported only for a cluster mode
    /// DISABLED group: the model says of each that it "is only applicable for
    /// Valkey or Redis OSS (cluster mode disabled)" groups, which are reached
    /// per node rather than through a configuration endpoint.
    pub fn to_modeled(
        &self,
        cluster_enabled: bool,
        clusters: &BTreeMap<String, CacheClusterRecord>,
    ) -> NodeGroupMember {
        NodeGroupMember {
            cache_cluster_id: Some(self.cache_cluster_id.clone()),
            cache_node_id: Some(self.cache_node_id.clone()),
            read_endpoint: if cluster_enabled {
                None
            } else {
                self.read_endpoint(clusters)
            },
            preferred_availability_zone: self.availability_zone.clone(),
            // Outposts are not modelled by this service; see
            // `CacheClusterRecord::to_modeled`.
            preferred_outpost_arn: None,
            current_role: (!cluster_enabled).then(|| self.current_role.clone()),
        }
    }

    /// The endpoint a client reads this member through: the endpoint of the one
    /// cache node of its member cluster, read out of that cluster's record so
    /// there is a single copy of it. It appears once the node is provisioned,
    /// exactly as `DescribeCacheClusters` reports the node itself.
    fn read_endpoint(&self, clusters: &BTreeMap<String, CacheClusterRecord>) -> Option<Endpoint> {
        clusters
            .get(&self.cache_cluster_id)?
            .nodes
            .iter()
            .find(|node| node.cache_node_id == self.cache_node_id)?
            .to_modeled()
            .endpoint
    }
}

/// One node group (shard) of a replication group, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct NodeGroupRecord {
    /// "The identifier for the node group (shard). A Valkey or Redis OSS
    /// (cluster mode disabled) replication group contains only 1 node group;
    /// therefore, the node group ID is 0001."
    pub node_group_id: String,
    /// One of `NodeGroup$Status`'s values ("creating, available, modifying,
    /// deleting").
    pub status: String,
    /// "The keyspace for this node group (shard)", reported for a cluster mode
    /// enabled group, where the keyspace is partitioned across shards.
    pub slots: Option<String>,
    /// This shard's members, the primary first.
    pub members: Vec<NodeGroupMemberRecord>,
}

impl NodeGroupRecord {
    /// This shard as the modelled `NodeGroup`.
    ///
    /// `PrimaryEndpoint` / `ReaderEndpoint` are the cluster mode DISABLED
    /// group's endpoints: a cluster mode enabled group is reached through the
    /// group's `ConfigurationEndpoint` instead. The reader endpoint exists only
    /// where there is a replica to read from.
    pub fn to_modeled(
        &self,
        group: &ReplicationGroupRecord,
        clusters: &BTreeMap<String, CacheClusterRecord>,
    ) -> NodeGroup {
        let per_node_endpoints = !group.cluster_enabled;
        let has_replica = self.members.len() > 1;
        NodeGroup {
            node_group_id: Some(self.node_group_id.clone()),
            status: Some(self.status.clone()),
            primary_endpoint: per_node_endpoints.then(|| Endpoint {
                address: Some(group.primary_endpoint_address()),
                port: Some(group.port),
            }),
            reader_endpoint: (per_node_endpoints && has_replica).then(|| Endpoint {
                address: Some(group.reader_endpoint_address()),
                port: Some(group.port),
            }),
            slots: self.slots.clone(),
            node_group_members: Some(
                self.members
                    .iter()
                    .map(|member| member.to_modeled(group.cluster_enabled, clusters))
                    .collect(),
            ),
        }
    }
}

/// One replication group, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct ReplicationGroupRecord {
    pub replication_group_id: String,
    pub description: String,
    pub region: String,
    pub arn: String,
    pub engine: String,
    pub engine_version: String,
    pub cache_node_type: Option<String>,
    /// One of `ReplicationGroup$Status`'s values ("creating, available,
    /// modifying, deleting, create-failed, snapshotting").
    pub status: String,
    /// The status change the clock owes this group, if any.
    pub pending_status: Option<PendingTransition>,
    pub created_at: SystemTime,
    /// The DNS label every endpoint of this group carries, minted once and
    /// stored, so an endpoint is stable across reads.
    pub dns_token: String,
    pub port: i32,
    /// `ClusterEnabled` — "whether its data can be partitioned across multiple
    /// shards (API/CLI: node groups)".
    pub cluster_enabled: bool,
    /// One of `ClusterMode`'s values (`enabled | disabled | compatible`).
    pub cluster_mode: String,
    /// [`SETTING_ENABLED`] or [`SETTING_DISABLED`].
    pub automatic_failover: String,
    /// See [`ReplicationGroupRecord::automatic_failover`].
    pub multi_az: String,
    pub node_groups: Vec<NodeGroupRecord>,
    pub preferred_maintenance_window: String,
    pub cache_subnet_group_name: Option<String>,
    pub cache_parameter_group_name: String,
    pub snapshot_retention_limit: Option<i32>,
    pub snapshot_window: Option<String>,
    /// Whether an `AuthToken` was set; the token itself is a SECRET and is NOT
    /// stored. See [`CacheClusterRecord::auth_token_enabled`].
    pub auth_token_enabled: bool,
    pub auth_token_last_modified: Option<SystemTime>,
    pub transit_encryption_enabled: bool,
    pub transit_encryption_mode: Option<String>,
    pub at_rest_encryption_enabled: bool,
    pub kms_key_id: Option<String>,
    /// [`SETTING_ENABLED`] or [`SETTING_DISABLED`].
    pub data_tiering: String,
    pub auto_minor_version_upgrade: Option<bool>,
    pub network_type: Option<String>,
    pub ip_discovery: Option<String>,
    pub user_group_ids: Vec<String>,
    pub log_delivery_configurations: Vec<LogDeliveryRecord>,
    /// One of `Durability`'s values (`default | async | sync | disabled`), as
    /// the request set it.
    pub durability: String,
    /// One of `EffectiveDurability`'s values (`async | sync | disabled`): what
    /// [`ReplicationGroupRecord::durability`] resolves to.
    pub effective_durability: String,
    /// The tags `CreateReplicationGroup` was given; see
    /// [`CacheClusterRecord::tags`].
    pub tags: Vec<(String, String)>,
}

impl ReplicationGroupRecord {
    /// The primary endpoint address of a cluster mode disabled group: the
    /// `master.` name a writer connects to.
    fn primary_endpoint_address(&self) -> String {
        format!(
            "master.{}.{}.{}.cache.amazonaws.com",
            self.replication_group_id, self.dns_token, self.region
        )
    }

    /// The reader endpoint address of a cluster mode disabled group: the
    /// `replica.` name a reader connects to.
    fn reader_endpoint_address(&self) -> String {
        format!(
            "replica.{}.{}.{}.cache.amazonaws.com",
            self.replication_group_id, self.dns_token, self.region
        )
    }

    /// The configuration endpoint address of a cluster mode enabled group.
    ///
    /// `CacheCluster$ConfigurationEndpoint` documents the shape of the name —
    /// "The configuration endpoint will always have `.cfg` in it" — which the
    /// `clustercfg` label a sharded group's endpoint carries satisfies.
    fn configuration_endpoint_address(&self) -> String {
        format!(
            "{}.{}.clustercfg.{}.cache.amazonaws.com",
            self.replication_group_id, self.dns_token, self.region
        )
    }

    /// What `StorageEncryptionType` reports: the kind of key the group's
    /// at-rest encryption uses, derived from whether it is enabled at all and
    /// whether the request named a KMS key.
    pub fn storage_encryption_type(&self) -> &'static str {
        if !self.at_rest_encryption_enabled {
            STORAGE_ENCRYPTION_NONE
        } else if self.kms_key_id.is_some() {
            STORAGE_ENCRYPTION_SSE_KMS
        } else {
            STORAGE_ENCRYPTION_SSE_ELASTICACHE
        }
    }

    /// "The names of all the cache clusters that are part of this replication
    /// group", derived from the shards rather than held a second time.
    pub fn member_clusters(&self) -> Vec<String> {
        let mut members: Vec<String> = self
            .node_groups
            .iter()
            .flat_map(|shard| shard.members.iter())
            .map(|member| member.cache_cluster_id.clone())
            .collect();
        members.sort();
        members.dedup();
        members
    }

    /// Materialise the status change the clock owes this group, if `now` is
    /// past its due instant. See [`CacheClusterRecord::settle`]: this is the one
    /// write a reporting operation may make.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(pending) = self.pending_status.as_ref() else {
            return;
        };
        if now < pending.due {
            return;
        }
        self.status = pending.status.clone();
        self.pending_status = None;
        for shard in &mut self.node_groups {
            shard.status = self.status.clone();
        }
    }

    /// This record as the modelled `ReplicationGroup`.
    ///
    /// `clusters` is the store's cluster map, which the members' read endpoints
    /// are read out of: a member cluster's endpoint is held once, on the cluster.
    pub fn to_modeled(
        &self,
        clusters: &BTreeMap<String, CacheClusterRecord>,
    ) -> ReplicationGroup {
        ReplicationGroup {
            replication_group_id: Some(self.replication_group_id.clone()),
            description: Some(self.description.clone()),
            // A Global datastore cannot exist here: every
            // global-replication-group operation is a scaffolded 501, so
            // `CreateReplicationGroup` refuses a `GlobalReplicationGroupId`
            // with the modelled not-found.
            global_replication_group_info: None,
            status: Some(self.status.clone()),
            // Nothing can be pending on a group this operation created:
            // `ModifyReplicationGroup` and the resharding and replica-count
            // operations are scaffolded 501s, and each owns its own issue.
            pending_modified_values: None,
            member_clusters: Some(self.member_clusters()),
            node_groups: Some(
                self.node_groups
                    .iter()
                    .map(|shard| shard.to_modeled(self, clusters))
                    .collect(),
            ),
            // The daily snapshot source: `CreateSnapshot` is a scaffolded 501,
            // so no member is snapshotting.
            snapshotting_cluster_id: None,
            automatic_failover: Some(self.automatic_failover.clone()),
            multi_az: Some(self.multi_az.clone()),
            // "Use the configuration endpoint to connect to this replication
            // group": the sharded group's one endpoint. A cluster mode disabled
            // group is reached through its shard's primary and reader
            // endpoints instead.
            configuration_endpoint: self.cluster_enabled.then(|| Endpoint {
                address: Some(self.configuration_endpoint_address()),
                port: Some(self.port),
            }),
            snapshot_retention_limit: self.snapshot_retention_limit,
            snapshot_window: self.snapshot_window.clone(),
            cluster_enabled: Some(self.cluster_enabled),
            cache_node_type: self.cache_node_type.clone(),
            auth_token_enabled: Some(self.auth_token_enabled),
            auth_token_last_modified_date: self
                .auth_token_last_modified
                .map(::aws_smithy_types::DateTime::from),
            transit_encryption_enabled: Some(self.transit_encryption_enabled),
            at_rest_encryption_enabled: Some(self.at_rest_encryption_enabled),
            // Outposts are not modelled by this service.
            member_clusters_outpost_arns: None,
            kms_key_id: self.kms_key_id.clone(),
            storage_encryption_type: Some(self.storage_encryption_type().to_string()),
            arn: Some(self.arn.clone()),
            user_group_ids: (!self.user_group_ids.is_empty())
                .then(|| self.user_group_ids.clone()),
            log_delivery_configurations: (!self.log_delivery_configurations.is_empty()).then(|| {
                self.log_delivery_configurations
                    .iter()
                    .map(LogDeliveryRecord::to_modeled)
                    .collect()
            }),
            replication_group_create_time: Some(::aws_smithy_types::DateTime::from(
                self.created_at,
            )),
            data_tiering: Some(self.data_tiering.clone()),
            auto_minor_version_upgrade: self.auto_minor_version_upgrade,
            network_type: self.network_type.clone(),
            ip_discovery: self.ip_discovery.clone(),
            transit_encryption_mode: self.transit_encryption_mode.clone(),
            cluster_mode: Some(self.cluster_mode.clone()),
            engine: Some(self.engine.clone()),
            durability: Some(self.durability.clone()),
            effective_durability: Some(self.effective_durability.clone()),
        }
    }
}

/// One member cluster a new replication group is to have: the cluster id, the
/// zone it is placed in, and whether it is an EXISTING cluster being adopted as
/// the group's primary (the `PrimaryClusterId` path) rather than minted.
#[derive(Debug, Clone)]
pub struct NewMember {
    pub cache_cluster_id: String,
    pub availability_zone: Option<String>,
    pub adopt: bool,
}

/// One node group (shard) a new replication group is to have.
#[derive(Debug, Clone)]
pub struct NewShard {
    pub node_group_id: String,
    pub slots: Option<String>,
    /// The shard's members, the primary first.
    pub members: Vec<NewMember>,
}

/// The validated values a new replication group is built from — one argument so
/// [`ComAmazonawsElasticacheAmazonelasticachev9State::insert_replication_group`]
/// does not take three dozen positional ones.
#[derive(Debug, Clone)]
pub struct NewReplicationGroup {
    pub replication_group_id: String,
    pub description: String,
    pub region: String,
    pub engine: String,
    pub engine_version: String,
    pub cache_node_type: Option<String>,
    pub port: i32,
    pub cluster_enabled: bool,
    pub cluster_mode: String,
    pub automatic_failover: bool,
    pub multi_az: bool,
    pub shards: Vec<NewShard>,
    pub preferred_maintenance_window: String,
    pub cache_subnet_group_name: Option<String>,
    pub cache_parameter_group_name: String,
    pub snapshot_retention_limit: Option<i32>,
    pub snapshot_window: Option<String>,
    pub auth_token_enabled: bool,
    pub transit_encryption_enabled: bool,
    pub transit_encryption_mode: Option<String>,
    pub at_rest_encryption_enabled: bool,
    pub kms_key_id: Option<String>,
    pub data_tiering: bool,
    pub auto_minor_version_upgrade: Option<bool>,
    pub security_group_ids: Vec<String>,
    pub notification_topic_arn: Option<String>,
    pub network_type: Option<String>,
    pub ip_discovery: Option<String>,
    pub user_group_ids: Vec<String>,
    pub log_delivery_configurations: Vec<LogDeliveryRecord>,
    pub durability: String,
    pub effective_durability: String,
    pub tags: Vec<(String, String)>,
    pub now: SystemTime,
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

/// One row of the cache-engine-version catalog this emulator publishes: an
/// engine and one version of it.
///
/// The row carries NO parameter group family: which family a version belongs to
/// is already stated once, by [`parameter_group_family`], and the catalog reads
/// it from there so the family a version reports and the default group a cluster
/// on that version joins cannot disagree.
#[derive(Debug, Clone, Copy)]
pub struct PublishedEngineVersion {
    /// One of [`ENGINES`] — "the name of the cache engine"
    /// (`CacheEngineVersion$Engine`).
    pub engine: &'static str,
    /// "The version number of the cache engine"
    /// (`CacheEngineVersion$EngineVersion`).
    pub engine_version: &'static str,
}

/// One catalog row, so the table below reads as a table.
const fn engine_version_row(
    engine: &'static str,
    engine_version: &'static str,
) -> PublishedEngineVersion {
    PublishedEngineVersion {
        engine,
        engine_version,
    }
}

/// The cache engines and versions this emulator publishes.
///
/// AWS publishes this catalog itself — the API has no operation that adds a
/// version — so the store holds it from the moment it comes up and
/// `DescribeCacheEngineVersions` reports it. Every row's version belongs to one
/// of the published [`PARAMETER_GROUP_FAMILIES`], which is where its
/// `CacheParameterGroupFamily` comes from.
///
/// The version numbers themselves are this emulator's published catalog rather
/// than a claim about what any one AWS region offers today; what the rest of the
/// service relies on is the SHAPE of it — several version lines per engine,
/// several versions per line, at least one version per published family, and a
/// newest version per engine, which is the one [`default_engine_version`] hands
/// a cluster that names none.
pub const CACHE_ENGINE_VERSIONS: [PublishedEngineVersion; 33] = [
    engine_version_row("memcached", "1.4.5"),
    // The version `EngineVersion`'s own documentation uses as its example.
    engine_version_row("memcached", "1.4.14"),
    engine_version_row("memcached", "1.4.24"),
    engine_version_row("memcached", "1.4.33"),
    engine_version_row("memcached", "1.4.34"),
    engine_version_row("memcached", "1.5.10"),
    engine_version_row("memcached", "1.5.16"),
    engine_version_row("memcached", "1.6.6"),
    engine_version_row("memcached", "1.6.12"),
    engine_version_row("memcached", "1.6.17"),
    engine_version_row("memcached", "1.6.22"),
    engine_version_row("redis", "2.6.13"),
    engine_version_row("redis", "2.8.6"),
    engine_version_row("redis", "2.8.19"),
    engine_version_row("redis", "2.8.22"),
    engine_version_row("redis", "2.8.23"),
    engine_version_row("redis", "2.8.24"),
    engine_version_row("redis", "3.2.4"),
    engine_version_row("redis", "3.2.6"),
    engine_version_row("redis", "3.2.10"),
    engine_version_row("redis", "4.0.10"),
    engine_version_row("redis", "5.0.0"),
    engine_version_row("redis", "5.0.3"),
    engine_version_row("redis", "5.0.4"),
    engine_version_row("redis", "5.0.5"),
    engine_version_row("redis", "5.0.6"),
    engine_version_row("redis", "6.0.5"),
    engine_version_row("redis", "6.2.5"),
    engine_version_row("redis", "6.2.6"),
    engine_version_row("redis", "7.0.7"),
    engine_version_row("redis", "7.1"),
    engine_version_row("valkey", "7.2"),
    engine_version_row("valkey", "8.0"),
];

/// How a published version's engine is described
/// (`CacheEngineVersion$CacheEngineDescription`: "the description of the cache
/// engine").
///
/// The three names the model itself uses for the engines, from
/// `CreateCacheCluster`'s own documentation: "All nodes in the cluster run the
/// same protocol-compliant cache engine software, either Memcached, Valkey or
/// Redis OSS."
pub fn engine_description(engine: &str) -> &'static str {
    match engine {
        "memcached" => "Memcached",
        "valkey" => "Valkey",
        _ => "Redis OSS",
    }
}

/// `version` with every numeric component zero-padded to a fixed width, so
/// comparing two of these byte-wise orders them the way version numbers compare
/// — `1.6.22` AFTER `1.6.6`, which the raw strings get backwards.
///
/// Four digits is the width: no cache engine has ever carried a version
/// component anywhere near 10000, and a component that wide would be the only
/// thing this ordering got wrong.
pub fn version_sort_key(version: &str) -> String {
    version
        .split('.')
        .map(|part| match part.parse::<u32>() {
            Ok(number) => format!("{number:04}"),
            // A component that is not a number (the `x` of an open `6.x` line)
            // sorts after every numbered one: `~` is above the digits in ASCII.
            Err(_) => format!("~{part}"),
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// The catalog key of `engine_version` of `engine`.
///
/// The engine name and then the version's sort key, so the map is ordered by
/// engine and, within an engine, by version from oldest to newest. That total
/// order is what the `Marker` indexes into, and its last row per engine is the
/// newest version of that engine.
pub fn engine_version_key(engine: &str, engine_version: &str) -> String {
    format!("{engine}|{}", version_sort_key(engine_version))
}

/// True when `version` is a version the filter value `filter` asks for: the same
/// version, or one in the version line it names (`6.2` names `6.2.5` and
/// `6.2.6`).
///
/// `EngineVersion` is "the cache engine version to return" with the example
/// `1.4.14`, and `DefaultOnly` speaks of "the specified engine or engine and
/// major version combination" — so a value that names only the leading
/// components of a version names that whole line.
pub fn version_matches(filter: &str, version: &str) -> bool {
    version == filter
        || version
            .strip_prefix(filter)
            .is_some_and(|rest| rest.starts_with('.'))
}

/// The highest version [`CACHE_ENGINE_VERSIONS`] publishes for `engine`, or
/// `None` when it publishes none for it.
fn latest_published_version(engine: &str) -> Option<&'static str> {
    CACHE_ENGINE_VERSIONS
        .iter()
        .filter(|row| row.engine == engine)
        .max_by_key(|row| version_sort_key(row.engine_version))
        .map(|row| row.engine_version)
}

/// One published cache engine version, as this emulator holds it.
#[derive(Debug, Clone)]
pub struct CacheEngineVersionRecord {
    /// See [`PublishedEngineVersion::engine`].
    pub engine: String,
    /// See [`PublishedEngineVersion::engine_version`].
    pub engine_version: String,
    /// The family the version belongs to, from [`parameter_group_family`] —
    /// absent for a version no published family covers, which is how
    /// `CacheEngineVersion$CacheParameterGroupFamily` reports "none" rather than
    /// inventing a family name.
    pub cache_parameter_group_family: Option<String>,
    /// See [`engine_description`].
    pub cache_engine_description: String,
    /// "The description of the cache engine version"
    /// (`CacheEngineVersion$CacheEngineVersionDescription`).
    pub cache_engine_version_description: String,
}

impl CacheEngineVersionRecord {
    /// This record as the modelled `CacheEngineVersion`.
    pub fn to_modeled(&self) -> CacheEngineVersion {
        CacheEngineVersion {
            engine: Some(self.engine.clone()),
            engine_version: Some(self.engine_version.clone()),
            cache_parameter_group_family: self.cache_parameter_group_family.clone(),
            cache_engine_description: Some(self.cache_engine_description.clone()),
            cache_engine_version_description: Some(self.cache_engine_version_description.clone()),
        }
    }
}

/// `ServerlessCache$Status`'s values this slice writes and reads: "The current
/// status of the serverless cache. The allowed values are CREATING, AVAILABLE,
/// DELETING, CREATE-FAILED and MODIFYING." The member is a plain `String`
/// rather than an enum, so these are the spellings. `DELETING` / `MODIFYING`
/// belong to `DeleteServerlessCache` / `ModifyServerlessCache`, which are still
/// scaffolded 501s, so they are spelled where those land rather than here.
pub const SERVERLESS_STATUS_CREATING: &str = "CREATING";
/// See [`SERVERLESS_STATUS_CREATING`].
pub const SERVERLESS_STATUS_AVAILABLE: &str = "AVAILABLE";

/// How long a serverless cache stays `CREATING` before the clock owes it
/// `AVAILABLE` — the same span a cache cluster's transitions take; see
/// [`CLUSTER_TRANSITION`] for why it is a chosen, non-zero span.
pub const SERVERLESS_CACHE_TRANSITION: Duration = CLUSTER_TRANSITION;

/// How many serverless caches the store holds before a create is refused with
/// `ServerlessCacheQuotaForCustomerExceededFault`. The emulator's OWN bound on
/// the store, for the reason [`CLUSTER_QUOTA`] gives.
pub const SERVERLESS_CACHE_QUOTA: usize = 150;

/// The largest `SnapshotRetentionLimit` a serverless cache may carry, as
/// `CreateServerlessCache$SnapshotRetentionLimit` documents it: "The maximum
/// value allowed is 35 days."
pub const MAX_SNAPSHOT_RETENTION_LIMIT: i32 = 35;

/// The bounds a `DataStorage` limit must be within, and the value that removes
/// the limit instead.
///
/// The member carries no modelled `@range`; the recorded refusal this repo
/// holds for an out-of-range limit
/// (`issues/.../20261022T193451Z-elasticache-nine-operation-families-answer-notimplemented-instead-of.md`,
/// recorded example "Invalid value for DataStorage limit. Must be between 1 and
/// 5000. You can set the value to zero to remove the limit.") is where these
/// come from.
pub const DATA_STORAGE_MIN_GB: i32 = 1;
/// See [`DATA_STORAGE_MIN_GB`].
pub const DATA_STORAGE_MAX_GB: i32 = 5000;
/// The `CacheUsageLimits` bound value that removes a limit rather than setting
/// one; see [`DATA_STORAGE_MIN_GB`].
pub const USAGE_LIMIT_REMOVED: i32 = 0;

/// The longest `Description` a serverless cache may carry, as
/// `CreateServerlessCache$Description` documents it: "The maximum length is 255
/// characters." The member carries no modelled `@length`, so the bound is
/// enforced in the handler.
pub const MAX_SERVERLESS_DESCRIPTION_LENGTH: usize = 255;

/// What `ServerlessCache$Description` reports for a cache created without one.
///
/// `CreateServerlessCache$Description` documents "The default is NULL, i.e. if
/// no description is provided then an empty string will be returned", and the
/// API normalises that empty description to a single space: a recorded create
/// that names no `Description` is answered with `" "`, and every later read of
/// the cache reports the same. The value is STORED, so the create response and
/// a later describe cannot disagree.
pub const DEFAULT_SERVERLESS_DESCRIPTION: &str = " ";

/// `ConnectionType`'s values (`smithy_query(query="shape",
/// shape="ConnectionType")`: `vpc | public`), and the default
/// `CreateServerlessCache$ConnectionType` documents: "If not specified,
/// defaults to vpc."
pub const CONNECTION_TYPE_VPC: &str = "vpc";
/// See [`CONNECTION_TYPE_VPC`].
pub const CONNECTION_TYPE_PUBLIC: &str = "public";

/// The engine, and the lowest major version of it, that a `public`
/// `ConnectionType` needs: "Setting this to public requires Valkey 9 or above"
/// (`CreateServerlessCache$ConnectionType`).
pub const PUBLIC_CONNECTION_ENGINE: &str = "valkey";
/// See [`PUBLIC_CONNECTION_ENGINE`].
pub const PUBLIC_CONNECTION_MIN_MAJOR: u32 = 9;

/// The `NetworkType` a serverless cache is created with when the request names
/// none: "If not specified, defaults to ipv4, unless all provided subnets are
/// IPv6-only, in which case it defaults to ipv6"
/// (`CreateServerlessCache$NetworkType`). The emulator models no EC2 subnets to
/// read an address family from, so the documented default applies.
pub const DEFAULT_NETWORK_TYPE: &str = "ipv4";

/// `StorageEncryptionType`'s values this slice uses, as
/// `ServerlessCache$StorageEncryptionType` documents them: "Serverless caches
/// are always encrypted at rest. The value is `sse-elasticache` if an
/// ElastiCache service-managed key is used, or `sse-kms` if a customer-managed
/// KMS key is used."
pub const STORAGE_ENCRYPTION_SSE_ELASTICACHE: &str = "sse-elasticache";
/// See [`STORAGE_ENCRYPTION_SSE_ELASTICACHE`].
pub const STORAGE_ENCRYPTION_SSE_KMS: &str = "sse-kms";

/// The Region in which a serverless cache that names no `SubnetIds` is given
/// two default subnets, and the number of default subnets everywhere else, as
/// `ServerlessCache$SubnetIds` documents it: "If no subnet IDs are given and
/// your VPC is in us-west-1, then ElastiCache will select 2 default subnets
/// across AZs in your VPC. For all other Regions, if no subnet IDs are given
/// then ElastiCache will select 3 default subnets across AZs in your default
/// VPC."
pub const TWO_DEFAULT_SUBNET_REGION: &str = "us-west-1";
/// See [`TWO_DEFAULT_SUBNET_REGION`].
pub const DEFAULT_SUBNETS_IN_TWO_SUBNET_REGION: usize = 2;
/// See [`TWO_DEFAULT_SUBNET_REGION`].
pub const DEFAULT_SUBNETS: usize = 3;

/// The page a `DescribeServerlessCaches` serves when the request names no
/// `MaxResults`: "The maximum number of records in the response. … The default
/// is 50." (`smithy_query(query="doc", shape="DescribeServerlessCachesRequest",
/// member="MaxResults")`).
pub const SERVERLESS_MAX_RESULTS_DEFAULT: i32 = 50;

/// The ARN of a serverless cache:
/// `arn:aws:elasticache:<region>:<account>:serverlesscache:<name>`.
pub fn serverless_cache_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("serverlesscache:{name}"))
}

/// How many default subnets a serverless cache created in `region` without
/// `SubnetIds` is given; see [`TWO_DEFAULT_SUBNET_REGION`].
pub fn default_subnet_count(region: &str) -> usize {
    if region == TWO_DEFAULT_SUBNET_REGION {
        DEFAULT_SUBNETS_IN_TWO_SUBNET_REGION
    } else {
        DEFAULT_SUBNETS
    }
}

/// The major engine version a serverless cache is created with when the request
/// names none: the major line of the engine's current version, which is the one
/// statement this emulator makes about what that version is (see
/// [`default_engine_version`]).
///
/// A line is the engine's version up to the component the parameter-group family
/// is named for — `<major>` for Valkey and Redis OSS, `<major>.<minor>` for
/// Memcached — exactly as [`default_parameter_group_name`] splits it.
pub fn default_major_engine_version(engine: &str) -> String {
    let version = default_engine_version(engine);
    let mut parts = version.split('.');
    let major = parts.next().unwrap_or_default();
    if engine == "memcached" {
        format!("{major}.{}", parts.next().unwrap_or("0"))
    } else {
        major.to_string()
    }
}

/// The full engine version a serverless cache compatible with major version
/// `major` runs, reported as `ServerlessCache$FullEngineVersion` ("The name and
/// version number of the engine the serverless cache is compatible with").
///
/// `DescribeCacheEngineVersions` is still a scaffolded 501, so the emulator's
/// own statement of the current version of each engine
/// ([`default_engine_version`]) is what a request for that engine's current
/// major line resolves to; any other line resolves to its first release.
pub fn serverless_full_engine_version(engine: &str, major: &str) -> String {
    let current = default_engine_version(engine);
    if current == major || current.starts_with(&format!("{major}.")) {
        current.to_string()
    } else {
        format!("{major}.0")
    }
}

/// True when `value` has the form of an engine version: dot-separated groups of
/// digits (`7`, `1.6`, `8.0`).
///
/// `MajorEngineVersion` carries no modelled constraint, so the boundary
/// validator passes any string and a value that cannot name a version is
/// refused here.
pub fn is_version_label(value: &str) -> bool {
    !value.is_empty()
        && value
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

/// The leading (major) component of a version label, or `None` when `value` is
/// not one.
pub fn major_version_number(value: &str) -> Option<u32> {
    value.split('.').next()?.parse().ok()
}

/// True when `value` is a daily snapshot time: `HH:MM` in UTC, the format
/// `DailySnapshotTime` takes ("The daily time that snapshots will be created
/// from the new serverless cache", the same `HH:MM` wall-clock form
/// `SnapshotWindow`'s "Example: 05:00-09:00" is built from).
pub fn is_daily_snapshot_time(value: &str) -> bool {
    let Some((hours, minutes)) = value.split_once(':') else {
        return false;
    };
    let two_digits = |part: &str| part.len() == 2 && part.bytes().all(|b| b.is_ascii_digit());
    if !two_digits(hours) || !two_digits(minutes) {
        return false;
    }
    matches!(hours.parse::<u32>(), Ok(h) if h < 24)
        && matches!(minutes.parse::<u32>(), Ok(m) if m < 60)
}

/// One `CacheUsageLimits` member as [`validate_cache_usage_limits`] checks it:
/// its name, its `Minimum`, its `Maximum`, and the upper bound its values must
/// be within (`None` where the API publishes none).
type UsageLimitBounds = (&'static str, Option<i32>, Option<i32>, Option<i32>);

/// Why a request's `CacheUsageLimits` cannot be stored, as the message the
/// refusal carries, or `Ok(())` when every limit is one this store may hold.
///
/// `DataStorage` / `ECPUPerSecond` carry no modelled `@range`, so the boundary
/// validator passes any integer and the bounds are enforced here: a
/// `DataStorage` bound must be within [`DATA_STORAGE_MIN_GB`] ..=
/// [`DATA_STORAGE_MAX_GB`], an `ECPUPerSecond` bound must be positive (the API
/// publishes no upper bound for it, so none is enforced), and either member
/// takes [`USAGE_LIMIT_REMOVED`] to remove the limit instead of setting one. A
/// `Minimum` above its own `Maximum` is refused for both. Every accepted value
/// is persisted EXACTLY as sent — the stored limits are what a later read
/// reports.
pub fn validate_cache_usage_limits(limits: &CacheUsageLimits) -> Result<(), String> {
    let mut members: Vec<UsageLimitBounds> = Vec::new();
    if let Some(storage) = limits.data_storage.as_ref() {
        members.push((
            "DataStorage",
            storage.minimum,
            storage.maximum,
            Some(DATA_STORAGE_MAX_GB),
        ));
    }
    if let Some(ecpu) = limits.ecpu_per_second.as_ref() {
        members.push(("ECPUPerSecond", ecpu.minimum, ecpu.maximum, None));
    }
    for (member, minimum, maximum, upper) in members {
        for value in [minimum, maximum].into_iter().flatten() {
            if value == USAGE_LIMIT_REMOVED {
                continue;
            }
            let within = value >= DATA_STORAGE_MIN_GB && upper.is_none_or(|upper| value <= upper);
            if !within {
                return Err(match upper {
                    Some(upper) => format!(
                        "Invalid value {value} for {member} limit. Must be between \
                         {DATA_STORAGE_MIN_GB} and {upper}. You can set the value to \
                         {USAGE_LIMIT_REMOVED} to remove the limit."
                    ),
                    None => format!(
                        "Invalid value {value} for {member} limit. Must be at least \
                         {DATA_STORAGE_MIN_GB}. You can set the value to {USAGE_LIMIT_REMOVED} to \
                         remove the limit."
                    ),
                });
            }
        }
        // A removed bound bounds nothing, so it cannot be the inverted one.
        if let (Some(minimum), Some(maximum)) = (minimum, maximum) {
            if minimum != USAGE_LIMIT_REMOVED
                && maximum != USAGE_LIMIT_REMOVED
                && minimum > maximum
            {
                return Err(format!(
                    "Invalid values for {member}. Minimum must not be greater than Maximum."
                ));
            }
        }
    }
    Ok(())
}

/// One serverless cache, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct ServerlessCacheRecord {
    /// The (lowercased) `ServerlessCacheName`, which "is stored as a lowercase
    /// string" and is the resource's identity in this API.
    pub name: String,
    pub region: String,
    /// What `ServerlessCache$Description` reports; see
    /// [`DEFAULT_SERVERLESS_DESCRIPTION`].
    pub description: String,
    pub engine: String,
    pub major_engine_version: String,
    pub full_engine_version: String,
    /// One of `ServerlessCache$Status`'s values.
    pub status: String,
    /// The status change the clock owes this cache, if any.
    pub pending_status: Option<PendingTransition>,
    pub created_at: SystemTime,
    /// The usage limits as the request set them, persisted unchanged.
    pub cache_usage_limits: Option<CacheUsageLimits>,
    pub kms_key_id: Option<String>,
    /// `sse-kms` for a customer-managed key, else `sse-elasticache`; see
    /// [`STORAGE_ENCRYPTION_SSE_ELASTICACHE`].
    pub storage_encryption_type: String,
    pub security_group_ids: Vec<String>,
    /// The subnets the cache's VPC endpoint is deployed in: the ones the request
    /// named, or the defaults ElastiCache selects; see [`DEFAULT_SUBNETS`].
    pub subnet_ids: Vec<String>,
    pub user_group_id: Option<String>,
    pub snapshot_retention_limit: Option<i32>,
    pub daily_snapshot_time: Option<String>,
    pub network_type: String,
    pub connection_type: String,
    pub endpoint_address: String,
    pub reader_endpoint_address: String,
    pub endpoint_port: i32,
    pub arn: String,
    /// The tags `CreateServerlessCache` was given; see
    /// [`CacheClusterRecord::tags`].
    pub tags: Vec<(String, String)>,
}

impl ServerlessCacheRecord {
    /// The endpoint address clients write to, and the one they read from.
    ///
    /// A serverless cache is reached through one DNS name per role rather than
    /// per node, so the name is the cache, this cache's stored DNS token, the
    /// `serverless` label, the Region and the service's DNS suffix — the same
    /// suffix [`CacheClusterRecord::configuration_endpoint_address`] builds. The
    /// reader endpoint is the same name with the `-ro` (read-only) suffix.
    fn endpoint_addresses(name: &str, dns_token: &str, region: &str) -> (String, String) {
        (
            format!("{name}-{dns_token}.serverless.{region}.cache.amazonaws.com"),
            format!("{name}-{dns_token}-ro.serverless.{region}.cache.amazonaws.com"),
        )
    }

    /// Materialise the status change the clock owes this cache, if `now` is past
    /// its due instant; see [`CacheClusterRecord::settle`].
    pub fn settle(&mut self, now: SystemTime) {
        let Some(pending) = self.pending_status.as_ref() else {
            return;
        };
        if now < pending.due {
            return;
        }
        self.status = pending.status.clone();
        self.pending_status = None;
    }

    /// This record as the modelled `ServerlessCache`.
    pub fn to_modeled(&self) -> ServerlessCache {
        ServerlessCache {
            serverless_cache_name: Some(self.name.clone()),
            description: Some(self.description.clone()),
            create_time: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            status: Some(self.status.clone()),
            engine: Some(self.engine.clone()),
            major_engine_version: Some(self.major_engine_version.clone()),
            full_engine_version: Some(self.full_engine_version.clone()),
            cache_usage_limits: self.cache_usage_limits.clone(),
            kms_key_id: self.kms_key_id.clone(),
            storage_encryption_type: Some(self.storage_encryption_type.clone()),
            security_group_ids: (!self.security_group_ids.is_empty())
                .then(|| self.security_group_ids.clone()),
            endpoint: Some(Endpoint {
                address: Some(self.endpoint_address.clone()),
                port: Some(self.endpoint_port),
            }),
            reader_endpoint: Some(Endpoint {
                address: Some(self.reader_endpoint_address.clone()),
                port: Some(self.endpoint_port),
            }),
            arn: Some(self.arn.clone()),
            user_group_id: self.user_group_id.clone(),
            subnet_ids: Some(self.subnet_ids.clone()),
            snapshot_retention_limit: self.snapshot_retention_limit,
            daily_snapshot_time: self.daily_snapshot_time.clone(),
            network_type: Some(self.network_type.clone()),
            connection_type: Some(self.connection_type.clone()),
            // A migration moves a self-designed cluster into a serverless
            // cache, and every migration operation (`StartMigration`,
            // `PrepareMigration`, `CompleteMigration`, …) is a scaffolded 501,
            // so no cache of this slice has one.
            migration: None,
        }
    }
}

/// One cache node, as the snapshot of its cluster holds it.
///
/// A snapshot is "a copy of an entire cluster as of the time when the snapshot
/// was taken", so these values are COPIED from the source node rather than read
/// back from it: a later `ModifyCacheCluster` cannot change what a snapshot
/// already taken reports.
#[derive(Debug, Clone)]
pub struct NodeSnapshotRecord {
    pub cache_cluster_id: String,
    pub cache_node_id: String,
    /// The node group (shard) the node belonged to. Always absent for a
    /// snapshot of a cache cluster, which belongs to no node group; it is what
    /// a replication-group snapshot would carry.
    pub node_group_id: Option<String>,
    /// The shard configuration `DescribeSnapshots$ShowNodeGroupConfig` asks
    /// for, absent for the same reason as [`NodeSnapshotRecord::node_group_id`].
    pub node_group_configuration: Option<NodeGroupConfiguration>,
    pub cache_node_create_time: SystemTime,
    pub snapshot_create_time: SystemTime,
}

impl NodeSnapshotRecord {
    /// This record as the modelled `NodeSnapshot`. The shard configuration is
    /// reported only when the caller asked for it
    /// (`DescribeSnapshots$ShowNodeGroupConfig`: "if true, the node group
    /// (shard) configuration is included in the snapshot description").
    pub fn to_modeled(&self, include_node_group_config: bool) -> NodeSnapshot {
        NodeSnapshot {
            cache_cluster_id: Some(self.cache_cluster_id.clone()),
            node_group_id: self.node_group_id.clone(),
            cache_node_id: Some(self.cache_node_id.clone()),
            node_group_configuration: include_node_group_config
                .then(|| self.node_group_configuration.clone())
                .flatten(),
            // "The size of the cache on the source cache node": this emulator
            // serves no cache data plane, so it holds no data whose size it
            // could report rather than invent.
            cache_size: None,
            cache_node_create_time: Some(::aws_smithy_types::DateTime::from(
                self.cache_node_create_time,
            )),
            snapshot_create_time: Some(::aws_smithy_types::DateTime::from(
                self.snapshot_create_time,
            )),
        }
    }
}

/// The published cache-engine-version catalog, keyed by
/// [`engine_version_key`].
///
/// A `BTreeMap` is what gives `DescribeCacheEngineVersions` the stable total
/// order its `Marker` indexes into, and the per-engine order its `DefaultOnly`
/// reads the newest version off.
pub fn published_cache_engine_versions() -> BTreeMap<String, CacheEngineVersionRecord> {
    CACHE_ENGINE_VERSIONS
        .iter()
        .map(|row| {
            let description = engine_description(row.engine);
            (
                engine_version_key(row.engine, row.engine_version),
                CacheEngineVersionRecord {
                    engine: row.engine.to_string(),
                    engine_version: row.engine_version.to_string(),
                    cache_parameter_group_family: parameter_group_family(
                        row.engine,
                        row.engine_version,
                    )
                    .map(str::to_string),
                    cache_engine_description: description.to_string(),
                    cache_engine_version_description: format!(
                        "{description} version {}",
                        row.engine_version
                    ),
                },
            )
        })
        .collect()
}

/// One snapshot, as this emulator persists it: the source cluster's
/// configuration as it stood when the snapshot was taken, plus the snapshot's
/// own identity and status.
#[derive(Debug, Clone)]
pub struct SnapshotRecord {
    /// The snapshot's identity, lowercased — `CreateSnapshot$SnapshotName` "is
    /// stored as a lowercase string".
    pub snapshot_name: String,
    pub cache_cluster_id: Option<String>,
    /// The source replication group. Always absent in this slice: every
    /// replication-group operation is a scaffolded 501, so no group can be
    /// snapshotted; it is what `DescribeSnapshots$ReplicationGroupId` filters
    /// on, so that filter reads stored state rather than a constant.
    pub replication_group_id: Option<String>,
    /// One of `Snapshot$SnapshotStatus`'s values (`creating | available |
    /// restoring | copying | deleting`), which spell the first two exactly as
    /// [`STATUS_CREATING`] and [`STATUS_AVAILABLE`] do.
    pub status: String,
    /// The status change the clock owes this snapshot, if any.
    pub pending_status: Option<PendingTransition>,
    /// [`SNAPSHOT_SOURCE_MANUAL`] or [`SNAPSHOT_SOURCE_AUTOMATED`].
    pub source: String,
    /// When the snapshot was taken. `Snapshot` has no member for it — each
    /// `NodeSnapshot$SnapshotCreateTime` reports it — so it is kept once here
    /// and copied onto the node snapshots.
    pub created_at: SystemTime,
    pub cache_node_type: Option<String>,
    pub engine: String,
    pub engine_version: String,
    pub num_cache_nodes: i32,
    pub preferred_availability_zone: Option<String>,
    pub cache_cluster_create_time: SystemTime,
    pub preferred_maintenance_window: String,
    pub topic_arn: Option<String>,
    pub port: i32,
    pub cache_parameter_group_name: String,
    pub cache_subnet_group_name: Option<String>,
    pub vpc_id: Option<String>,
    pub auto_minor_version_upgrade: Option<bool>,
    /// The source cluster's `SnapshotRetentionLimit` when the snapshot was
    /// taken: "For manual snapshots, this field reflects the
    /// SnapshotRetentionLimit for the source cluster when the snapshot was
    /// created. This field is otherwise ignored: Manual snapshots do not
    /// expire."
    pub snapshot_retention_limit: Option<i32>,
    pub snapshot_window: Option<String>,
    pub kms_key_id: Option<String>,
    pub arn: String,
    pub node_snapshots: Vec<NodeSnapshotRecord>,
    /// The tags `CreateSnapshot` was given. `AddTagsToResource` /
    /// `ListTagsForResource` are scaffolded 501s and own their issues; the tags
    /// are persisted here so implementing them does not have to move them.
    pub tags: Vec<(String, String)>,
}

impl SnapshotRecord {
    /// A snapshot of `cluster` as it stands now.
    ///
    /// Every reported value is COPIED out of the stored cluster record, which is
    /// what makes the snapshot "a copy of an entire cluster as of the time when
    /// the snapshot was taken" rather than a second view of the live cluster.
    pub fn from_cluster(cluster: &CacheClusterRecord, new: NewSnapshot) -> Self {
        SnapshotRecord {
            arn: snapshot_arn(&cluster.region, &new.snapshot_name),
            node_snapshots: cluster
                .nodes
                .iter()
                .map(|node| NodeSnapshotRecord {
                    cache_cluster_id: cluster.cache_cluster_id.clone(),
                    cache_node_id: node.cache_node_id.clone(),
                    node_group_id: None,
                    node_group_configuration: None,
                    cache_node_create_time: node.create_time,
                    snapshot_create_time: new.now,
                })
                .collect(),
            snapshot_name: new.snapshot_name,
            cache_cluster_id: Some(cluster.cache_cluster_id.clone()),
            replication_group_id: cluster.replication_group_id.clone(),
            status: STATUS_CREATING.to_string(),
            pending_status: Some(PendingTransition {
                due: new.now + SNAPSHOT_TRANSITION,
                status: STATUS_AVAILABLE.to_string(),
            }),
            source: new.source,
            created_at: new.now,
            cache_node_type: cluster.cache_node_type.clone(),
            engine: cluster.engine.clone(),
            engine_version: cluster.engine_version.clone(),
            num_cache_nodes: cluster.num_cache_nodes,
            preferred_availability_zone: cluster.preferred_availability_zone(),
            cache_cluster_create_time: cluster.created_at,
            preferred_maintenance_window: cluster.preferred_maintenance_window.clone(),
            topic_arn: cluster.notification_topic_arn.clone(),
            port: cluster.endpoint_port,
            cache_parameter_group_name: cluster.cache_parameter_group_name.clone(),
            cache_subnet_group_name: cluster.cache_subnet_group_name.clone(),
            vpc_id: new.vpc_id,
            auto_minor_version_upgrade: cluster.auto_minor_version_upgrade,
            snapshot_retention_limit: cluster.snapshot_retention_limit,
            snapshot_window: cluster.snapshot_window.clone(),
            kms_key_id: new.kms_key_id,
            tags: new.tags,
        }
    }

    /// Materialise the status change the clock owes this snapshot, if `now` is
    /// past its due instant. The one write a reporting operation may make; see
    /// [`CacheClusterRecord::settle`].
    pub fn settle(&mut self, now: SystemTime) {
        let Some(pending) = self.pending_status.as_ref() else {
            return;
        };
        if now < pending.due {
            return;
        }
        self.status = pending.status.clone();
        self.pending_status = None;
    }

    /// This record as the modelled `Snapshot`.
    pub fn to_modeled(&self, include_node_group_config: bool) -> Snapshot {
        Snapshot {
            snapshot_name: Some(self.snapshot_name.clone()),
            replication_group_id: self.replication_group_id.clone(),
            // The description of the source replication group: a snapshot of a
            // cache cluster has no source group to take one from.
            replication_group_description: None,
            cache_cluster_id: self.cache_cluster_id.clone(),
            snapshot_status: Some(self.status.clone()),
            snapshot_source: Some(self.source.clone()),
            cache_node_type: self.cache_node_type.clone(),
            engine: Some(self.engine.clone()),
            engine_version: Some(self.engine_version.clone()),
            num_cache_nodes: Some(self.num_cache_nodes),
            preferred_availability_zone: self.preferred_availability_zone.clone(),
            // Outposts: `CreateCacheCluster` refuses every Outpost member, so
            // no cluster here was placed in one.
            preferred_outpost_arn: None,
            cache_cluster_create_time: Some(::aws_smithy_types::DateTime::from(
                self.cache_cluster_create_time,
            )),
            preferred_maintenance_window: Some(self.preferred_maintenance_window.clone()),
            topic_arn: self.topic_arn.clone(),
            port: Some(self.port),
            cache_parameter_group_name: Some(self.cache_parameter_group_name.clone()),
            cache_subnet_group_name: self.cache_subnet_group_name.clone(),
            vpc_id: self.vpc_id.clone(),
            auto_minor_version_upgrade: self.auto_minor_version_upgrade,
            snapshot_retention_limit: self.snapshot_retention_limit,
            snapshot_window: self.snapshot_window.clone(),
            // The number of node groups (shards) and the automatic-failover
            // status are the source replication group's; a cache cluster's
            // snapshot carries neither.
            num_node_groups: None,
            automatic_failover: None,
            node_snapshots: Some(
                self.node_snapshots
                    .iter()
                    .map(|node| node.to_modeled(include_node_group_config))
                    .collect(),
            ),
            kms_key_id: self.kms_key_id.clone(),
            arn: Some(self.arn.clone()),
            // Data tiering is "only supported for replication groups using the
            // r6gd node type" and durability is set on a cluster this API
            // cannot create, so no snapshot here was taken with either.
            data_tiering: None,
            durability: None,
        }
    }
}

/// The validated values a new serverless cache is built from — one argument so
/// [`ComAmazonawsElasticacheAmazonelasticachev9State::insert_serverless_cache`]
/// does not take two dozen positional strings.
#[derive(Debug, Clone)]
pub struct NewServerlessCache {
    pub name: String,
    pub region: String,
    pub description: String,
    pub engine: String,
    pub major_engine_version: String,
    pub cache_usage_limits: Option<CacheUsageLimits>,
    pub kms_key_id: Option<String>,
    pub security_group_ids: Vec<String>,
    pub subnet_ids: Vec<String>,
    pub user_group_id: Option<String>,
    pub snapshot_retention_limit: Option<i32>,
    pub daily_snapshot_time: Option<String>,
    pub network_type: String,
    pub connection_type: String,
    pub port: i32,
    pub tags: Vec<(String, String)>,
    pub now: SystemTime,
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

/// The values a new snapshot carries that do not come from the source cluster —
/// one argument so [`SnapshotRecord::from_cluster`] does not take a row of
/// positional strings.
#[derive(Debug, Clone)]
pub struct NewSnapshot {
    /// Already lowercased, as the stored name is.
    pub snapshot_name: String,
    /// [`SNAPSHOT_SOURCE_MANUAL`] for a `CreateSnapshot` call.
    pub source: String,
    pub kms_key_id: Option<String>,
    /// The VPC of the source cluster's subnet group, resolved by the caller
    /// (which holds the store) rather than guessed here.
    pub vpc_id: Option<String>,
    pub tags: Vec<(String, String)>,
    pub now: SystemTime,
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

// ---------------------------------------------------------------------------
// RBAC users
// ---------------------------------------------------------------------------

/// The cache engines an RBAC user may be created for, as `CreateUser$Engine`
/// documents it: "The options are valkey or redis". RBAC is a Valkey / Redis
/// OSS feature, so `memcached` — a legal CLUSTER engine, see [`ENGINES`] — is
/// not one of them.
pub const USER_ENGINES: [&str; 2] = ["redis", "valkey"];

/// `engine` lowercased, or the reason it names no engine a user can run on.
///
/// ElastiCache stores engine names in lowercase, so the comparison and the
/// stored value are both lowercase — the same rule [`normalize_engine`] applies
/// to a cluster's engine, over the narrower set a user permits.
pub fn normalize_user_engine(engine: &str) -> Result<String, String> {
    let lowered = engine.to_ascii_lowercase();
    if USER_ENGINES.contains(&lowered.as_str()) {
        Ok(lowered)
    } else {
        Err(format!(
            "Invalid value {engine} for Engine: must be one of {}.",
            USER_ENGINES.join(" | ")
        ))
    }
}

/// `User$Status` of a user the service has accepted: "Indicates the user status.
/// Can be \"active\", \"modifying\" or \"deleting\"". A created user is usable at
/// once — ElastiCache answers `CreateUser` with it already active — so there is
/// no transition for the clock to owe; only a later modify or delete moves it.
pub const USER_STATUS_ACTIVE: &str = "active";

/// How many passwords one user may hold, as `CreateUser$Passwords` documents it:
/// "Passwords used for this user. You can create up to two passwords for each
/// user."
pub const MAX_PASSWORDS_PER_USER: usize = 2;

/// How many users the store holds before a create is refused with the modelled
/// `UserQuotaExceededFault`. The emulator's OWN bound on the store, for the
/// reason [`CLUSTER_QUOTA`] gives.
pub const USER_QUOTA: usize = 100;

/// `User$MinimumEngineVersion` of a user that authenticates with a password or
/// without one: "The minimum engine version required, which is Redis OSS 6.0" —
/// the version RBAC itself arrived in.
pub const USER_MIN_ENGINE_VERSION: &str = "6.0";

/// `User$MinimumEngineVersion` of a user that authenticates with IAM:
/// `InputAuthenticationType` accepts `iam`, and ElastiCache supports IAM
/// authentication for RBAC users only from Redis OSS 7.0 onwards, so such a user
/// cannot be attached to a cache running an older engine.
pub const USER_MIN_ENGINE_VERSION_IAM: &str = "7.0";

/// `AuthenticationType`'s values (`smithy_query(query="shape",
/// shape="AuthenticationType")`: `password | no-password | iam`), named so the
/// handlers branch on a constant rather than a literal. The full accepted set
/// stays the GENERATED catalog `crate::ops::types::AUTHENTICATION_TYPE_VALUES`.
pub mod authentication_type {
    pub const PASSWORD: &str = "password";
    pub const NO_PASSWORD: &str = "no-password";
    pub const IAM: &str = "iam";
}

/// `InputAuthenticationType`'s values — what an `AuthenticationMode` may ASK for
/// (`smithy_query(query="shape", shape="InputAuthenticationType")`: `password |
/// no-password-required | iam`). They are NOT the same tokens a user reports
/// back (`no-password-required` is asked for, `no-password` is reported).
pub mod input_authentication_type {
    pub const PASSWORD: &str = "password";
    pub const NO_PASSWORD_REQUIRED: &str = "no-password-required";
    pub const IAM: &str = "iam";
}

/// The user every ElastiCache account already has.
///
/// ElastiCache creates one RBAC user, with the id and the name `default`, in
/// every account; it is the user a cache serves unauthenticated traffic as and
/// the one every user group must contain. No call creates it, so the store is
/// stocked with it when it is built — see [`built_in_users`] — and no reporting
/// operation has to write it into existence.
pub const DEFAULT_USER_ID: &str = "default";

/// The access string that built-in user carries: every key (`~*`), every pub/sub
/// channel (`&*`) and every command (`+@all`), in the Valkey / Redis OSS ACL
/// syntax `User$AccessString` is written in.
pub const DEFAULT_USER_ACCESS_STRING: &str = "on ~* &* +@all";

/// The `User` members a `DescribeUsers` `Filter` may name, as `Filter$Name`
/// documents it: "The property being filtered. For example, UserId." A filter
/// names a property of the resource being reported, so the accepted set is the
/// scalar members of the `User` shape.
pub const USER_FILTER_NAMES: [&str; 7] = [
    "UserId",
    "UserName",
    "Engine",
    "Status",
    "AccessString",
    "MinimumEngineVersion",
    "ARN",
];

/// The ARN of an RBAC user: `arn:aws:elasticache:<region>:<account>:user:<id>`.
pub fn user_arn(region: &str, user_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("user:{user_id}"))
}

/// The ACL rule keywords a Valkey / Redis OSS access string may carry on their
/// own — the rules that take no argument.
///
/// `AccessString` is "Access permissions string used for this user", written in
/// the engine's own ACL syntax (the syntax `DEFAULT_USER_ACCESS_STRING` is one
/// example of), and the engine's keyword rules are the catalog below. Rules that
/// take an argument (`~`/`%`/`&` patterns and `+`/`-` command rules) are
/// recognised by [`is_access_rule`] instead.
const ACCESS_STRING_KEYWORDS: [&str; 14] = [
    "on",
    "off",
    "nopass",
    "resetpass",
    "allkeys",
    "allchannels",
    "allcommands",
    "nocommands",
    "resetkeys",
    "resetchannels",
    "clearselectors",
    "reset",
    "sanitize-payload",
    "skip-sanitize-payload",
];

/// True when `rule` is one rule of a Valkey / Redis OSS ACL rule list:
///
/// * a keyword from [`ACCESS_STRING_KEYWORDS`];
/// * a key pattern `~<glob>`, or a read/write-qualified one `%R~`/`%W~`/`%RW~`;
/// * a pub/sub channel pattern `&<glob>`;
/// * a command rule `+<command>` / `-<command>`, or a category rule
///   `+@<category>` / `-@<category>`.
///
/// A selector (`(+get ~key1)`) is a rule list in parentheses rather than a
/// single rule, so [`access_string_rejection`] groups it and checks its inner
/// rules through here.
fn is_access_rule(rule: &str) -> bool {
    if ACCESS_STRING_KEYWORDS.contains(&rule) {
        return true;
    }
    // A key or channel pattern: the glob itself is any non-empty run of
    // non-whitespace, because the engine matches it verbatim.
    if let Some(glob) = rule.strip_prefix('~').or_else(|| rule.strip_prefix('&')) {
        return !glob.is_empty();
    }
    // `%RW~<glob>`: the qualifier is a non-empty run of `R`/`W` before the `~`.
    if let Some(rest) = rule.strip_prefix('%') {
        let Some((qualifier, glob)) = rest.split_once('~') else {
            return false;
        };
        return !qualifier.is_empty()
            && qualifier
                .bytes()
                .all(|byte| byte.eq_ignore_ascii_case(&b'r') || byte.eq_ignore_ascii_case(&b'w'))
            && !glob.is_empty();
    }
    let Some(rest) = rule
        .strip_prefix('+')
        .or_else(|| rule.strip_prefix('-'))
    else {
        return false;
    };
    match rest.strip_prefix('@') {
        // A category name: letters and hyphens (`@read`, `@sanitize-payload`).
        Some(category) => {
            !category.is_empty()
                && category
                    .bytes()
                    .all(|byte| byte.is_ascii_alphabetic() || byte == b'-')
        }
        // A command, optionally one subcommand of it (`+config|get`).
        None => {
            rest.starts_with(|first: char| first.is_ascii_alphabetic())
                && rest
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'|' || byte == b'-')
        }
    }
}

/// Why `access_string` is not an access string, or `None` when it is one.
///
/// The model constrains `AccessString` only with `@pattern "\S"` — any string
/// carrying a non-space character passes the boundary validator — so the rule
/// list itself is parsed here, and a request naming something that is not an ACL
/// rule is refused instead of being stored as a permission set the engine could
/// never apply. The caller maps the refusal onto its own modelled
/// `InvalidParameterValueException`.
///
/// A SELECTOR — the Redis OSS 7 / Valkey form `(+get ~key1)` — is one rule that
/// carries a rule list of its own, so it is read as a parenthesised group whose
/// inner rules are checked by the same per-rule test. A group that is never
/// closed is refused.
pub fn access_string_rejection(member: &str, access_string: &str) -> Option<String> {
    let refuse = |token: &str| {
        Some(format!(
            "Invalid value {token} for {member}: it is not a Valkey or Redis OSS ACL rule."
        ))
    };
    let mut rules = 0usize;
    // The selector being read, once its `(` has been seen and before its `)`.
    let mut selector: Option<String> = None;
    for token in access_string.split_whitespace() {
        if let Some(group) = selector.as_mut() {
            group.push(' ');
            group.push_str(token);
            if !token.ends_with(')') {
                continue;
            }
            let group = selector.take().unwrap_or_default();
            if !is_selector(&group) {
                return refuse(&group);
            }
            rules += 1;
            continue;
        }
        if token.starts_with('(') {
            if token.ends_with(')') && token.len() > 1 {
                if !is_selector(token) {
                    return refuse(token);
                }
                rules += 1;
            } else {
                selector = Some(token.to_string());
            }
            continue;
        }
        if !is_access_rule(token) {
            return refuse(token);
        }
        rules += 1;
    }
    if let Some(unterminated) = selector {
        return refuse(&unterminated);
    }
    if rules == 0 {
        return Some(format!("Invalid value for {member}: it names no access rule."));
    }
    None
}

/// True when `group` is a selector: a parenthesised, non-empty rule list, every
/// rule of which is one [`is_access_rule`] accepts.
fn is_selector(group: &str) -> bool {
    let Some(inner) = group
        .strip_prefix('(')
        .and_then(|rest| rest.strip_suffix(')'))
    else {
        return false;
    };
    let mut rules = inner.split_whitespace().peekable();
    rules.peek().is_some() && rules.all(is_access_rule)
}

/// An access string as the service STORES it: the rules it names, one space
/// apart.
///
/// Only the whitespace between rules is normalised — no rule is added, removed
/// or reordered — so what a later read reports is the permission set the request
/// asked for.
pub fn canonical_access_string(access_string: &str) -> String {
    access_string.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Why a request's authentication members cannot make a user's `Authentication`.
#[derive(Debug, Clone)]
pub enum AuthRefusal {
    /// A member's value is outside what the service accepts — the caller's
    /// modelled `InvalidParameterValueException`.
    Value(String),
    /// The members supplied cannot hold together — the caller's modelled
    /// `InvalidParameterCombinationException`.
    Combination(String),
}

/// How a stored user authenticates: `Authentication`'s "Type" and, for a
/// password user, "The number of passwords belonging to the user".
///
/// The passwords THEMSELVES are never stored: ElastiCache reports only the count
/// (`Authentication$PasswordCount`), and a secret the API never reads back has no
/// business sitting in emulator state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticationRecord {
    /// An `AuthenticationType` token — see [`authentication_type`].
    pub r#type: String,
    /// How many passwords the user holds, for a password user; `None` for one
    /// that authenticates with IAM or without a password.
    pub password_count: Option<i32>,
}

impl AuthenticationRecord {
    /// The user's `Authentication`, as the API reports it.
    pub fn to_modeled(&self) -> Authentication {
        Authentication {
            r#type: Some(self.r#type.clone()),
            password_count: self.password_count,
        }
    }

    /// The `MinimumEngineVersion` a user authenticating this way requires.
    pub fn minimum_engine_version(&self) -> &'static str {
        if self.r#type == authentication_type::IAM {
            USER_MIN_ENGINE_VERSION_IAM
        } else {
            USER_MIN_ENGINE_VERSION
        }
    }
}

/// The `Authentication` the passwords in `passwords` give a user, or why they
/// are refused.
fn password_authentication(passwords: &[String]) -> Result<AuthenticationRecord, AuthRefusal> {
    if passwords.len() > MAX_PASSWORDS_PER_USER {
        return Err(AuthRefusal::Value(format!(
            "{} passwords were supplied; you can create up to {MAX_PASSWORDS_PER_USER} passwords \
             for each user.",
            passwords.len()
        )));
    }
    // At most `MAX_PASSWORDS_PER_USER`, refused above, so the conversion is
    // exact; the fallback only keeps the handler panic-free.
    let count = i32::try_from(passwords.len()).unwrap_or(i32::MAX);
    Ok(AuthenticationRecord {
        r#type: authentication_type::PASSWORD.to_string(),
        password_count: Some(count),
    })
}

/// The `Authentication` a request's authentication members describe, or why they
/// are refused.
///
/// A user authenticates in exactly ONE way, so the three members that can say
/// which are mutually exclusive: `AuthenticationMode` ("Specifies how to
/// authenticate the user") states the whole of it on its own, while the older
/// `Passwords` ("Passwords used for this user") and `NoPasswordRequired`
/// ("Indicates a password is not required for this user") are the two halves of
/// the same choice. A request that names none of them has not said how the user
/// authenticates and is refused rather than given a default the caller did not
/// ask for.
///
/// `CreateUser` and `ModifyUser` take the same members, so both resolve them
/// here and cannot drift about which combinations they refuse.
pub fn authentication_from_request(
    passwords: Option<&[String]>,
    no_password_required: Option<bool>,
    mode: Option<&AuthenticationMode>,
) -> Result<AuthenticationRecord, AuthRefusal> {
    let passwords = passwords.unwrap_or(&[]);
    if let Some(mode) = mode {
        if !passwords.is_empty() || no_password_required.is_some() {
            return Err(AuthRefusal::Combination(
                "AuthenticationMode cannot be combined with Passwords or NoPasswordRequired: \
                 either states how the user authenticates on its own."
                    .to_string(),
            ));
        }
        let mode_passwords = mode.passwords.as_deref().unwrap_or(&[]);
        return match mode.r#type.as_deref().unwrap_or_default() {
            input_authentication_type::PASSWORD => {
                if mode_passwords.is_empty() {
                    return Err(AuthRefusal::Combination(format!(
                        "AuthenticationMode.Passwords is required when AuthenticationMode.Type \
                         is {}.",
                        input_authentication_type::PASSWORD
                    )));
                }
                password_authentication(mode_passwords)
            }
            token @ (input_authentication_type::NO_PASSWORD_REQUIRED
            | input_authentication_type::IAM) => {
                if !mode_passwords.is_empty() {
                    return Err(AuthRefusal::Combination(format!(
                        "AuthenticationMode.Passwords cannot be supplied when \
                         AuthenticationMode.Type is {token}."
                    )));
                }
                Ok(AuthenticationRecord {
                    // `no-password-required` is what a request ASKS for;
                    // `no-password` is what the user reports back.
                    r#type: if token == input_authentication_type::IAM {
                        authentication_type::IAM.to_string()
                    } else {
                        authentication_type::NO_PASSWORD.to_string()
                    },
                    password_count: None,
                })
            }
            other => Err(AuthRefusal::Value(format!(
                "AuthenticationMode.Type must be one of {} — got {other:?}.",
                // The GENERATED catalog, so a refusal cannot name a different
                // set from the one the model declares. `InputAuthenticationType`
                // IS a modelled enum, so the boundary validator normally refuses
                // an unknown token before a handler runs; this arm is what keeps
                // an absent `Type` from being read as one of the three.
                crate::ops::types::INPUT_AUTHENTICATION_TYPE_VALUES.join(", ")
            ))),
        };
    }
    if no_password_required == Some(true) {
        if !passwords.is_empty() {
            return Err(AuthRefusal::Combination(
                "Passwords cannot be supplied when NoPasswordRequired is true.".to_string(),
            ));
        }
        return Ok(AuthenticationRecord {
            r#type: authentication_type::NO_PASSWORD.to_string(),
            password_count: None,
        });
    }
    if !passwords.is_empty() {
        return password_authentication(passwords);
    }
    Err(AuthRefusal::Combination(
        "No password was supplied and NoPasswordRequired was not set: name Passwords, \
         NoPasswordRequired or an AuthenticationMode."
            .to_string(),
    ))
}

/// One stored RBAC user — the one copy `CreateUser` writes and every user
/// operation reads.
#[derive(Debug, Clone)]
pub struct UserRecord {
    /// `UserId`, as stored: "This value is stored as a lowercase string".
    pub user_id: String,
    pub user_name: String,
    /// The engine, lowercased — see [`normalize_user_engine`].
    pub engine: String,
    /// The ACL rule list, as [`canonical_access_string`] stores it.
    pub access_string: String,
    pub authentication: AuthenticationRecord,
    /// A `User$Status` token — see [`USER_STATUS_ACTIVE`].
    pub status: String,
    pub arn: String,
    /// The tags the create was given, for `ListTagsForResource` (still a
    /// scaffolded 501 with its own issue) to report.
    pub tags: Vec<(String, String)>,
}

impl UserRecord {
    /// The user as the API reports it — `CreateUser`'s output shape is the `User`
    /// shape, so this is also what `DescribeUsers` answers with.
    pub fn to_modeled(&self) -> CreateUserOutput {
        CreateUserOutput {
            user_id: Some(self.user_id.clone()),
            user_name: Some(self.user_name.clone()),
            status: Some(self.status.clone()),
            engine: Some(self.engine.clone()),
            minimum_engine_version: Some(
                self.authentication.minimum_engine_version().to_string(),
            ),
            access_string: Some(self.access_string.clone()),
            // "Returns a list of the user group IDs the user belongs to":
            // membership is held by the user GROUPS that list the user, and every
            // user-group operation is still a scaffolded 501 (each with its own
            // issue), so no group can list a user yet and the list is empty
            // rather than invented. When those land they derive it from the group
            // records instead of a second copy kept here.
            user_group_ids: Some(Vec::new()),
            authentication: Some(self.authentication.to_modeled()),
            arn: Some(self.arn.clone()),
        }
    }

    /// The value of the `User` property a `DescribeUsers` `Filter` names, or
    /// `None` when the name is not one of [`USER_FILTER_NAMES`].
    pub fn property(&self, name: &str) -> Option<&str> {
        match name {
            "UserId" => Some(self.user_id.as_str()),
            "UserName" => Some(self.user_name.as_str()),
            "Engine" => Some(self.engine.as_str()),
            "Status" => Some(self.status.as_str()),
            "AccessString" => Some(self.access_string.as_str()),
            "MinimumEngineVersion" => Some(self.authentication.minimum_engine_version()),
            "ARN" => Some(self.arn.as_str()),
            _ => None,
        }
    }
}

/// The users a store holds before any call of this API: the built-in `default`
/// user ElastiCache gives every account.
///
/// Stocked when the store is BUILT, like the service-update and offering
/// catalogs, so no reporting operation has to write the built-in into existence
/// and no request's region can decide what its ARN says. The ARN therefore names
/// [`DEFAULT_REGION`]: the user is not the product of any request, so there is no
/// request region to take.
pub fn built_in_users() -> BTreeMap<String, UserRecord> {
    let mut users = BTreeMap::new();
    users.insert(
        DEFAULT_USER_ID.to_string(),
        UserRecord {
            user_id: DEFAULT_USER_ID.to_string(),
            user_name: DEFAULT_USER_ID.to_string(),
            // The built-in user predates Valkey support, so it runs the engine
            // RBAC arrived on.
            engine: DEFAULT_ENGINE.to_string(),
            access_string: DEFAULT_USER_ACCESS_STRING.to_string(),
            authentication: AuthenticationRecord {
                r#type: authentication_type::NO_PASSWORD.to_string(),
                password_count: None,
            },
            status: USER_STATUS_ACTIVE.to_string(),
            arn: user_arn(DEFAULT_REGION, DEFAULT_USER_ID),
            tags: Vec::new(),
        },
    );
    users
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
    /// Every stored replication group, keyed by its (lowercased)
    /// `ReplicationGroupId`, ordered for the same reason: it is what gives
    /// `DescribeReplicationGroups` the stable total order its `Marker` indexes
    /// into. A group's member clusters live in
    /// [`Self::cache_clusters`] beside the standalone ones, so there is one copy
    /// of each cluster however it was created.
    pub replication_groups: BTreeMap<String, ReplicationGroupRecord>,
    /// Every cache parameter group, keyed by its (lowercased) name, ordered for
    /// the same reason. Unlike the maps above this one is SEEDED when the store
    /// is created, with the default group of every published family (see
    /// [`default_parameter_groups`]): those groups exist in an account before
    /// anything has been created, and seeding them HERE rather than from a
    /// handler is what keeps `DescribeCacheParameterGroups` a pure read.
    pub cache_parameter_groups: BTreeMap<String, CacheParameterGroupRecord>,
    /// Every stored snapshot, keyed by its (lowercased) `SnapshotName` — the
    /// resource's identity in this API, which `CreateSnapshot$SnapshotName`
    /// "is stored as a lowercase string". Ordered for the same reason as
    /// [`ComAmazonawsElasticacheAmazonelasticachev9State::cache_clusters`]:
    /// `DescribeSnapshots`'s `Marker` indexes into that total order.
    pub snapshots: BTreeMap<String, SnapshotRecord>,
    /// The service updates this emulator publishes, keyed by
    /// `ServiceUpdateName` and ordered for the same reason. AWS releases these
    /// rather than an account creating them, so the store holds the catalog
    /// from the moment it comes up — see [`published_service_updates`].
    pub service_updates: BTreeMap<String, ServiceUpdateRecord>,
    /// Every stored RBAC user, keyed by its (lowercased) `UserId` — the
    /// resource's identity in this API — including the built-in `default` user
    /// [`built_in_users`] stocks the store with. A `BTreeMap`, so `DescribeUsers`
    /// has the stable total order its `Marker` indexes into.
    pub users: BTreeMap<String, UserRecord>,
    /// The cache engine versions this emulator publishes, keyed by
    /// [`engine_version_key`]. AWS publishes these rather than an account
    /// creating them, so the store holds the catalog from the moment it comes
    /// up — see [`published_cache_engine_versions`].
    pub cache_engine_versions: BTreeMap<String, CacheEngineVersionRecord>,
    /// Every stored serverless cache, keyed by its (lowercased)
    /// `ServerlessCacheName`, ordered for the same reason.
    pub serverless_caches: BTreeMap<String, ServerlessCacheRecord>,
    /// The counter endpoint DNS labels are minted from.
    pub dns_token_seq: u64,
    /// The counter VPC ids are minted from.
    pub vpc_seq: u64,
    /// The counter the default subnet ids of a serverless cache are minted from.
    pub subnet_seq: u64,
    /// The reserved-cache-node offerings this service publishes, keyed for the
    /// same stable total order. Stocked from [`offering_catalog`] when the store
    /// is created, because the catalog is the service's own and no operation
    /// creates or removes an entry: a describe only reports it.
    pub reserved_cache_nodes_offerings: BTreeMap<String, ReservedCacheNodesOffering>,
    /// Every reservation this account bought, keyed by its
    /// `ReservedCacheNodeId` — "the unique identifier for the reservation",
    /// which is "a customer-specified identifier" and so is held exactly as it
    /// was given. A `BTreeMap` is what gives `DescribeReservedCacheNodes` the
    /// stable total order its `Marker` indexes into.
    pub reserved_cache_nodes: BTreeMap<String, ReservedCacheNodeRecord>,
}

impl Default for ComAmazonawsElasticacheAmazonelasticachev9State {
    /// A fresh store is an account ElastiCache has just become available in: no
    /// customer resources, the default parameter group of every family the
    /// service publishes, and the catalogs this service publishes rather than
    /// an account creating them — the service updates and the
    /// reserved-cache-node offerings.
    ///
    /// The engine builds this the first time a handler of this service asks for
    /// its state, so that instant is the anchor the service-update catalog's
    /// release and end dates hang off — fixed for the life of the process, which
    /// is what makes every later read of an update report the same dates.
    fn default() -> Self {
        Self {
            cache_clusters: BTreeMap::new(),
            cache_subnet_groups: BTreeMap::new(),
            replication_groups: BTreeMap::new(),
            serverless_caches: BTreeMap::new(),
            cache_parameter_groups: default_parameter_groups(),
            snapshots: BTreeMap::new(),
            service_updates: published_service_updates(SystemTime::now()),
            users: built_in_users(),
            cache_engine_versions: published_cache_engine_versions(),
            dns_token_seq: 0,
            vpc_seq: 0,
            subnet_seq: 0,
            reserved_cache_nodes_offerings: offering_catalog(),
            reserved_cache_nodes: BTreeMap::new(),
        }
    }
}

impl ComAmazonawsElasticacheAmazonelasticachev9State {
    /// The parameter group `name` names, or `None` when this account holds no
    /// such group — the one lookup every handler that resolves a
    /// `CacheParameterGroupName` goes through, so they cannot disagree about
    /// which groups exist. The name "is stored as a lowercase string", so the
    /// lookup is case-insensitive.
    pub fn parameter_group(&self, name: &str) -> Option<&CacheParameterGroupRecord> {
        self.cache_parameter_groups.get(&name.to_ascii_lowercase())
    }

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

    /// Mint the id of one subnet ElastiCache selects for a serverless cache the
    /// request gave no `SubnetIds`: `subnet-` and eight lowercase hex digits,
    /// the form EC2 uses. The emulator models no EC2 subnets to read one from,
    /// so the selected ids are minted once and STORED on the record, which is
    /// what makes every later read report the same subnets.
    pub fn mint_subnet_id(&mut self) -> String {
        self.subnet_seq = self.subnet_seq.wrapping_add(1);
        format!("subnet-{:08x}", self.subnet_seq & 0xffff_ffff)
    }

    /// Build and store a serverless-cache record, returning what was stored.
    ///
    /// Takes the already-validated values so the one place a serverless cache is
    /// created is also the one place its endpoints, ARN and encryption type are
    /// derived from them. A created cache is NOT ready: it answers `CREATING`,
    /// and the clock owes it `AVAILABLE` once
    /// [`SERVERLESS_CACHE_TRANSITION`] has passed.
    pub fn insert_serverless_cache(&mut self, new: NewServerlessCache) -> &ServerlessCacheRecord {
        let dns_token = self.mint_dns_token();
        let (endpoint_address, reader_endpoint_address) =
            ServerlessCacheRecord::endpoint_addresses(&new.name, &dns_token, &new.region);
        let record = ServerlessCacheRecord {
            arn: serverless_cache_arn(&new.region, &new.name),
            name: new.name.clone(),
            region: new.region,
            description: new.description,
            full_engine_version: serverless_full_engine_version(
                &new.engine,
                &new.major_engine_version,
            ),
            engine: new.engine,
            major_engine_version: new.major_engine_version,
            status: SERVERLESS_STATUS_CREATING.to_string(),
            pending_status: Some(PendingTransition {
                due: new.now + SERVERLESS_CACHE_TRANSITION,
                status: SERVERLESS_STATUS_AVAILABLE.to_string(),
            }),
            created_at: new.now,
            cache_usage_limits: new.cache_usage_limits,
            storage_encryption_type: if new.kms_key_id.is_some() {
                STORAGE_ENCRYPTION_SSE_KMS.to_string()
            } else {
                STORAGE_ENCRYPTION_SSE_ELASTICACHE.to_string()
            },
            kms_key_id: new.kms_key_id,
            security_group_ids: new.security_group_ids,
            subnet_ids: new.subnet_ids,
            user_group_id: new.user_group_id,
            snapshot_retention_limit: new.snapshot_retention_limit,
            daily_snapshot_time: new.daily_snapshot_time,
            network_type: new.network_type,
            connection_type: new.connection_type,
            endpoint_address,
            reader_endpoint_address,
            endpoint_port: new.port,
            tags: new.tags,
        };
        // The handler refuses a name the store already holds, under the same
        // lock this insert takes, so the slot is always vacant here. The
        // response is read back out of the map, so it is built from the record
        // this service persisted rather than from a copy beside it.
        debug_assert!(
            !self.serverless_caches.contains_key(&new.name),
            "a serverless cache is only inserted once"
        );
        self.serverless_caches.entry(new.name).or_insert(record)
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
            replication_group_id: new.replication_group_id,
            replication_group_log_delivery_enabled: new.replication_group_log_delivery_enabled,
            cache_parameter_group_name: new.cache_parameter_group_name,
            snapshot_retention_limit: new.snapshot_retention_limit,
            snapshot_window: new.snapshot_window,
            auto_minor_version_upgrade: new.auto_minor_version_upgrade,
            security_group_ids: new.security_group_ids,
            notification_topic_arn: new.notification_topic_arn,
            auth_token_enabled: new.auth_token_enabled,
            auth_token_last_modified: new.auth_token_enabled.then_some(new.now),
            transit_encryption_enabled: new.transit_encryption_enabled,
            transit_encryption_mode: new.transit_encryption_mode,
            at_rest_encryption_enabled: new.at_rest_encryption_enabled,
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

    /// The offering `id` names, or `None` when the published catalog holds no
    /// such entry — the one lookup every handler that resolves a
    /// `ReservedCacheNodesOfferingId` goes through, so they cannot disagree
    /// about which offerings exist. The id is opaque, so it is matched exactly.
    pub fn offering(&self, id: &str) -> Option<&ReservedCacheNodesOffering> {
        self.reserved_cache_nodes_offerings
            .values()
            .find(|offering| offering.reserved_cache_nodes_offering_id.as_deref() == Some(id))
    }

    /// How many cache nodes this account's reservations already hold, which is
    /// what [`RESERVED_CACHE_NODE_QUOTA`] bounds.
    pub fn reserved_node_count(&self) -> i64 {
        self.reserved_cache_nodes
            .values()
            .map(|reservation| i64::from(reservation.cache_node_count))
            .sum()
    }

    /// Mint the identifier of a reservation whose purchase named none.
    ///
    /// [`generated_reservation_id`] names it after the purchase instant; two
    /// purchases inside the same rendered instant would otherwise collide, so
    /// the first free sequence is appended to keep a generated id unique.
    pub fn mint_reservation_id(&self, at: SystemTime) -> String {
        let base = generated_reservation_id(at);
        if !self.reserved_cache_nodes.contains_key(&base) {
            return base;
        }
        (1..=RESERVED_CACHE_NODE_QUOTA)
            .map(|seq| format!("{base}-{seq}"))
            .find(|candidate| !self.reserved_cache_nodes.contains_key(candidate))
            // Unreachable: the store holds at most
            // [`RESERVED_CACHE_NODE_QUOTA`] reservations, so one of the
            // candidates above is always free.
            .unwrap_or(base)
    }

    /// Store `record` and hand back what was stored, so a purchase's response
    /// is built from the persisted reservation rather than from a copy beside
    /// it.
    pub fn insert_reservation(
        &mut self,
        record: ReservedCacheNodeRecord,
    ) -> &ReservedCacheNodeRecord {
        match self
            .reserved_cache_nodes
            .entry(record.reserved_cache_node_id.clone())
        {
            Entry::Vacant(slot) => slot.insert(record),
            Entry::Occupied(mut slot) => {
                slot.insert(record);
                slot.into_mut()
            }
        }
    }

    /// Store `record` and hand back what was stored, so a create's response is
    /// built from the persisted record rather than from a copy beside it.
    pub fn insert_snapshot(&mut self, record: SnapshotRecord) -> &SnapshotRecord {
        match self.snapshots.entry(record.snapshot_name.clone()) {
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
        for group in self.replication_groups.values_mut() {
            group.settle(now);
        }
        for cache in self.serverless_caches.values_mut() {
            cache.settle(now);
        }
        for snapshot in self.snapshots.values_mut() {
            snapshot.settle(now);
        }
        for reservation in self.reserved_cache_nodes.values_mut() {
            reservation.settle(now);
        }
    }

    /// Build and store a replication group and the member clusters it is made
    /// of; the caller reads it back with [`Self::modeled_replication_group`].
    ///
    /// Every member cluster is a real cluster record in [`Self::cache_clusters`]
    /// — minted here, or, for the `PrimaryClusterId` path, the existing cluster
    /// joined to the group — so `DescribeCacheClusters` reports exactly the
    /// clusters the group is made of.
    pub fn insert_replication_group(&mut self, new: NewReplicationGroup) {
        let dns_token = self.mint_dns_token();
        let log_delivery_enabled = !new.log_delivery_configurations.is_empty();
        let mut node_groups = Vec::with_capacity(new.shards.len());
        for shard in &new.shards {
            let mut members = Vec::with_capacity(shard.members.len());
            for (index, member) in shard.members.iter().enumerate() {
                if member.adopt {
                    // The cluster the request named as the group's primary: it
                    // keeps its own configuration and joins the group.
                    if let Some(cluster) = self.cache_clusters.get_mut(&member.cache_cluster_id) {
                        cluster.replication_group_id = Some(new.replication_group_id.clone());
                        cluster.replication_group_log_delivery_enabled =
                            Some(log_delivery_enabled);
                    }
                } else {
                    let _ = self.insert_cluster(NewCluster {
                        cache_cluster_id: member.cache_cluster_id.clone(),
                        region: new.region.clone(),
                        engine: new.engine.clone(),
                        engine_version: new.engine_version.clone(),
                        cache_node_type: new.cache_node_type.clone(),
                        // A Valkey / Redis OSS cluster holds exactly one node;
                        // a shard's replicas are further member CLUSTERS, not
                        // further nodes of one.
                        num_cache_nodes: REDIS_NUM_CACHE_NODES,
                        port: new.port,
                        availability_zones: member
                            .availability_zone
                            .clone()
                            .into_iter()
                            .collect(),
                        az_mode: None,
                        preferred_maintenance_window: new.preferred_maintenance_window.clone(),
                        cache_subnet_group_name: new.cache_subnet_group_name.clone(),
                        cache_parameter_group_name: new.cache_parameter_group_name.clone(),
                        snapshot_retention_limit: new.snapshot_retention_limit,
                        snapshot_window: new.snapshot_window.clone(),
                        auto_minor_version_upgrade: new.auto_minor_version_upgrade,
                        security_group_ids: new.security_group_ids.clone(),
                        notification_topic_arn: new.notification_topic_arn.clone(),
                        auth_token_enabled: new.auth_token_enabled,
                        transit_encryption_enabled: Some(new.transit_encryption_enabled),
                        transit_encryption_mode: new.transit_encryption_mode.clone(),
                        at_rest_encryption_enabled: Some(new.at_rest_encryption_enabled),
                        network_type: new.network_type.clone(),
                        ip_discovery: new.ip_discovery.clone(),
                        log_delivery_configurations: new.log_delivery_configurations.clone(),
                        // ElastiCache tags a replication group's member
                        // clusters with the group's tags, so a later
                        // `ListTagsForResource` on a member answers them.
                        tags: new.tags.clone(),
                        now: new.now,
                        replication_group_id: Some(new.replication_group_id.clone()),
                        replication_group_log_delivery_enabled: Some(log_delivery_enabled),
                    });
                }
                members.push(NodeGroupMemberRecord {
                    cache_cluster_id: member.cache_cluster_id.clone(),
                    cache_node_id: FIRST_CACHE_NODE_ID.to_string(),
                    availability_zone: member.availability_zone.clone(),
                    current_role: if index == 0 { ROLE_PRIMARY } else { ROLE_REPLICA }.to_string(),
                });
            }
            node_groups.push(NodeGroupRecord {
                node_group_id: shard.node_group_id.clone(),
                status: STATUS_CREATING.to_string(),
                slots: shard.slots.clone(),
                members,
            });
        }

        let record = ReplicationGroupRecord {
            arn: replication_group_arn(&new.region, &new.replication_group_id),
            replication_group_id: new.replication_group_id.clone(),
            description: new.description,
            region: new.region,
            engine: new.engine,
            engine_version: new.engine_version,
            cache_node_type: new.cache_node_type,
            // A created group is NOT ready: it answers `creating`, and the
            // clock owes it `available` once the transition span has passed.
            status: STATUS_CREATING.to_string(),
            pending_status: Some(PendingTransition {
                due: new.now + CLUSTER_TRANSITION,
                status: STATUS_AVAILABLE.to_string(),
            }),
            created_at: new.now,
            dns_token,
            port: new.port,
            cluster_enabled: new.cluster_enabled,
            cluster_mode: new.cluster_mode,
            automatic_failover: if new.automatic_failover {
                SETTING_ENABLED
            } else {
                SETTING_DISABLED
            }
            .to_string(),
            multi_az: if new.multi_az {
                SETTING_ENABLED
            } else {
                SETTING_DISABLED
            }
            .to_string(),
            node_groups,
            preferred_maintenance_window: new.preferred_maintenance_window,
            cache_subnet_group_name: new.cache_subnet_group_name,
            cache_parameter_group_name: new.cache_parameter_group_name,
            snapshot_retention_limit: new.snapshot_retention_limit,
            snapshot_window: new.snapshot_window,
            auth_token_enabled: new.auth_token_enabled,
            auth_token_last_modified: new.auth_token_enabled.then_some(new.now),
            transit_encryption_enabled: new.transit_encryption_enabled,
            transit_encryption_mode: new.transit_encryption_mode,
            at_rest_encryption_enabled: new.at_rest_encryption_enabled,
            kms_key_id: new.kms_key_id,
            data_tiering: if new.data_tiering {
                SETTING_ENABLED
            } else {
                SETTING_DISABLED
            }
            .to_string(),
            auto_minor_version_upgrade: new.auto_minor_version_upgrade,
            network_type: new.network_type,
            ip_discovery: new.ip_discovery,
            user_group_ids: new.user_group_ids,
            log_delivery_configurations: new.log_delivery_configurations,
            durability: new.durability,
            effective_durability: new.effective_durability,
            tags: new.tags,
        };
        self.replication_groups
            .insert(new.replication_group_id.clone(), record);
    }

    /// The stored replication group `id` names, as the modelled shape — built
    /// from what the store holds rather than from a copy beside it.
    pub fn modeled_replication_group(&self, id: &str) -> Option<ReplicationGroup> {
        Some(
            self.replication_groups
                .get(id)?
                .to_modeled(&self.cache_clusters),
        )
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
    /// See [`CacheClusterRecord::transit_encryption_mode`].
    pub transit_encryption_mode: Option<String>,
    /// See [`CacheClusterRecord::at_rest_encryption_enabled`].
    pub at_rest_encryption_enabled: Option<bool>,
    pub network_type: Option<String>,
    pub ip_discovery: Option<String>,
    pub log_delivery_configurations: Vec<LogDeliveryRecord>,
    pub tags: Vec<(String, String)>,
    pub now: SystemTime,
    /// The replication group this cluster is a member of, if any; see
    /// [`CacheClusterRecord::replication_group_id`].
    pub replication_group_id: Option<String>,
    /// See [`CacheClusterRecord::replication_group_log_delivery_enabled`].
    pub replication_group_log_delivery_enabled: Option<bool>,
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
