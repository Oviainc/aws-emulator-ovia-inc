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
//! `CreateServerlessCache` mints one, `ModifyServerlessCache` changes it,
//! `DescribeServerlessCaches` reports it and `DeleteServerlessCache` takes it
//! away. Its record and its projection into the modelled `ServerlessCache`
//! shape live here too, so those handlers write the same
//! copy the reader reads instead of introducing a
//! second one. A deleted cache does not vanish at the call: it answers
//! `DELETING` and the clock owes its removal
//! ([`ServerlessCacheRecord::pending_deletion`]), the same way a created one is
//! owed `AVAILABLE`.
//!
//! A serverless cache's snapshot is its own resource, held in
//! [`ServerlessCacheSnapshotRecord`] beside the caches rather than on them,
//! because it outlives the cache it was taken of:
//! `CreateServerlessCacheSnapshot` mints one from a stored serverless cache (and
//! `DeleteServerlessCache`'s `FinalSnapshotName` mints the last one),
//! `DescribeServerlessCacheSnapshots` reports it, and the copy, export and
//! delete calls for one are still scaffolded 501s that will read and write this
//! same copy rather than a second one.
//!
//! A snapshot is a copy of a cluster as of the time it was taken:
//! `CreateSnapshot` mints one from a stored cluster, `CopySnapshot` mints a
//! second from a stored snapshot, `DescribeSnapshots` reports it and
//! `DeleteSnapshot` takes it away, so [`SnapshotRecord`] below is the one copy
//! all of them read. A deleted snapshot does not vanish at the call: it answers
//! `deleting` and the clock owes its removal
//! ([`SnapshotRecord::pending_removal`]), the same way a created one is owed
//! `available`.
//!
//! Beside the resources sits the event log: every change this service makes to
//! one of them records an [`EventRecord`], and `DescribeEvents` reports that log
//! over the window a request asks for. The log is the one copy, kept beside the
//! resources rather than on them, because an event outlives the resource it is
//! about and because a status change the CLOCK materialises
//! ([`CacheClusterRecord::settle`]) has to record one too.
//!
//! A resource's TAGS, by contrast, live ON its record
//! ([`CacheClusterRecord::tags`] and its siblings), because a tag belongs to one
//! resource and goes when it goes. That is the one copy a create's `Tags` member
//! writes, `ListTagsForResource` reports and `AddTagsToResource` /
//! `RemoveTagsFromResource` change; the three of them resolve an ARN to it
//! through [`ComAmazonawsElasticacheAmazonelasticachev9State::resource_tags`], so
//! they cannot disagree about which resource an ARN names.
//!
//! Beside those customer-created resources sit the catalogs this service
//! publishes rather than mints: the service updates AWS releases for its cache
//! engines, the cache engines and versions themselves, and the ENGINE DEFAULT
//! PARAMETERS of every cache parameter group family
//! ([`published_engine_default_parameters`]). No operation creates or removes an
//! entry of any of them (the API has no such call), so the store starts out
//! holding each catalog and `DescribeServiceUpdates` /
//! `DescribeCacheEngineVersions` / `DescribeEngineDefaultParameters` only report
//! it. The engine-version catalog is also the one statement of which version is
//! the newest of each engine and which parameter group family a version belongs
//! to, so `CreateCacheCluster`'s defaults are read off it rather than from a
//! second list.
//!
//! The reserved-cache-node offerings are a further such catalog, and the
//! reservations bought from one ([`ReservedCacheNodeRecord`]) ARE an account
//! resource: `PurchaseReservedCacheNodesOffering` mints one and
//! `DescribeReservedCacheNodes` reports it.
//!
//! A cache parameter group is a supporting resource:
//! `CreateCacheParameterGroup` mints a custom one and
//! `DescribeCacheParameterGroups` reports it beside the family defaults the
//! service publishes. The parameter VALUES a group has had SET on it live on
//! that record ([`CacheParameterGroupRecord::parameter_values`]), because a
//! group reports its family's engine defaults with exactly those values laid
//! over them — which is what `DescribeCacheParameters` answers with.
//! `ModifyCacheParameterGroup` is the operation that WRITES that map and
//! `ResetCacheParameterGroup` the one that removes entries from it, so a
//! parameter a group no longer holds a value for reports the engine's own
//! default again. Keeping the map on the group record is what lets the writers
//! write the copy the reader already reads instead of a second one beside it.
//!
//! Beside the cache resources sits the RBAC user: `CreateUser` mints one,
//! `ModifyUser` changes its permissions and the way it authenticates,
//! `DescribeUsers` reports it and `DeleteUser` takes it away, over the one
//! [`UserRecord`] map below, which also holds the built-in `default` user every
//! account already has. A modified user does not settle at the call: it answers
//! `modifying` and the clock owes it `active` ([`UserRecord::pending_status`])
//! the same way a deleted one is owed its removal.
//!
//! The update action is this service's one resource nothing creates: it exists
//! for every (published service update, cluster-or-replication-group) pair the
//! update applies to, so `DescribeUpdateActions` crosses the catalog with
//! [`ComAmazonawsElasticacheAmazonelasticachev9State::update_action_targets`]
//! and reports a pair with no [`UpdateActionRecord`] as `not-applied`;
//! `BatchApplyUpdateAction` and `BatchStopUpdateAction` are what store a record
//! for one, and the clock carries it from there.
//!
//! A user group is the RBAC resource built out of those users:
//! `CreateUserGroup` mints one, `DescribeUserGroups` reports it and
//! `ModifyUserGroup` changes the users that belong to it and the engine it is
//! listed for. A membership change is not applied at the call — the group
//! answers `modifying` and carries the change on
//! [`UserGroupRecord::pending_user_ids_to_add`] /
//! [`UserGroupRecord::pending_user_ids_to_remove`], which is what
//! `UserGroup$PendingChanges` reports until the clock applies it
//! ([`UserGroupRecord::settle`]) — so the membership still has ONE copy, the
//! group's `user_ids`, and `User$UserGroupIds` stays derived from it.
//!
//! Everything else in this service (`DeleteUserGroup`, the migration
//! operations other than `StartMigration`, and the parameter-group operations
//! that would change a parameter's value) is still a scaffolded 501 and owns its
//! own gap issue; where a member of an implemented operation belongs to one of
//! those resources, the comment beside it says so.
//!
//! A data migration is the work that moves a customer-run cache into one this
//! account runs: `StartMigration` starts the data sync into the target the
//! request names — a replication group or a serverless cache — and the ONE copy
//! of the open migration lives on that target's record
//! ([`ServerlessCacheRecord::migration`] /
//! [`ReplicationGroupRecord::migration`], both a [`MigrationRecord`]). That one
//! copy is what refuses a second start on the same target and what
//! `ServerlessCache$Migration` reports; `PrepareMigration`, `StopMigration`,
//! `CompleteMigration` and `TestMigration` are still scaffolded 501s with their
//! own gap issues and read and write the same record rather than a copy beside
//! it.
//!
//! The serverless cache is another customer-created resource this store holds:
//! `CreateServerlessCache` mints one, `ModifyServerlessCache` changes it,
//! `DescribeServerlessCaches` reports it and `DeleteServerlessCache` removes it.
//! The serverless snapshot operations are still scaffolded 501s with their own
//! gap issues, and every one of them reads and writes the SAME record rather
//! than a copy beside it.
//!
//! A replication group is the other resource this state holds:
//! `CreateReplicationGroup` mints one together with the member cache clusters it
//! is made of, and `DescribeReplicationGroups` reports it. Those member clusters
//! are ordinary [`CacheClusterRecord`]s in the same map as the standalone ones,
//! so there is one copy of a cluster however it was created.
//!
//! A Global datastore ([`GlobalReplicationGroupRecord`]) is built AROUND one
//! such replication group: `CreateGlobalReplicationGroup` mints it from the
//! primary that accepts writes, `DescribeGlobalReplicationGroups` reports it,
//! `ModifyGlobalReplicationGroup`, the failover, the (dis)association and the
//! slice-group operations change it, and `DeleteGlobalReplicationGroup` takes it
//! away. Which datastore a replication group belongs to and the role it has
//! there is held ONCE per record on each side — on the datastore's member list
//! and on [`ReplicationGroupRecord::global_replication_group_id`] — and every
//! operation that moves a membership writes both, so the two reads cannot
//! disagree about what the account runs.

use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    Authentication, AuthenticationMode, AvailabilityZone, CacheCluster, CacheEngineVersion,
    CacheNode, CacheNodeTypeSpecificParameter, CacheNodeTypeSpecificValue, CacheParameterGroup,
    CacheNodeUpdateStatus, CacheParameterGroupStatus, CacheSubnetGroup, CacheUsageLimits,
    ConfigureShard, CreateUserGroupOutput, CreateUserOutput, CustomerNodeEndpoint, DataStorage,
    DestinationDetails, ECPUPerSecond, Endpoint, Event, MigrationSummary,
    GlobalNodeGroup, GlobalReplicationGroup, GlobalReplicationGroupInfo,
    GlobalReplicationGroupMember, LogDeliveryConfiguration, LogDeliveryConfigurationRequest,
    NodeGroup, NodeGroupConfiguration, NodeGroupMember, NodeGroupMemberUpdateStatus,
    NodeGroupUpdateStatus, NodeSnapshot, NotificationConfiguration, Parameter,
    PendingLogDeliveryConfiguration, PendingModifiedValues, ProcessedUpdateAction, RecurringCharge,
    ReplicationGroup, ReplicationGroupPendingModifiedValues, ReservedCacheNode,
    ReservedCacheNodesOffering, ScaleConfig, SecurityGroupMembership, ServerlessCache,
    ServerlessCacheConfiguration, ServerlessCacheSnapshot,
    ServiceUpdate, Snapshot, Subnet, Tag, UnprocessedUpdateAction, UpdateAction,
    UserGroupPendingChanges, UserGroupsUpdateStatus,
};

pub mod kms_key;

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

/// The most nodes this emulator will MATERIALISE for one cluster, whatever a
/// request asks for: the largest cluster any engine permits
/// ([`MEMCACHED_MAX_NODES`]).
///
/// The handlers refuse a larger `NumCacheNodes` before a record is built (see
/// [`validate_num_cache_nodes`]); this is the bound at the allocation site
/// itself, so no path through [`CacheClusterRecord::resize_nodes`] can be made
/// to allocate node records without end.
pub const MAX_CACHE_NODES_PER_CLUSTER: i32 = MEMCACHED_MAX_NODES;

/// The smallest cluster: a cluster always has at least one node, which is also
/// the floor [`CacheClusterRecord::resize_nodes`] clamps to.
pub const MIN_CACHE_NODES_PER_CLUSTER: i32 = 1;
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

/// The day abbreviations a maintenance window may name, in week order.
///
/// `PreferredMaintenanceWindow` states them: "Valid values for `ddd` are: sun,
/// mon, tue, wed, thu, fri, sat". The position in this list is the day's offset
/// from the start of the week, which is what makes the span of a window that
/// crosses midnight computable ([`maintenance_window_refusal`]).
pub const MAINTENANCE_WINDOW_DAYS: [&str; 7] =
    ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

/// The documented minimum span of a maintenance window, in minutes:
/// `PreferredMaintenanceWindow` states "The minimum maintenance window is a 60
/// minute period."
pub const MAINTENANCE_WINDOW_MIN_MINUTES: i64 = 60;

/// Minutes in a day, and in a week: the arithmetic a `ddd:hh24:mi` point is
/// placed on.
const MINUTES_PER_DAY: i64 = 24 * 60;
/// See [`MINUTES_PER_DAY`].
const MINUTES_PER_WEEK: i64 = 7 * MINUTES_PER_DAY;

/// How ElastiCache words a `PreferredMaintenanceWindow` it cannot read.
///
/// The format is the one the member documents — "a range in the format
/// ddd:hh24:mi-ddd:hh24:mi (24H Clock UTC)" — and this is the wording AWS was
/// recorded answering with. The HTTP status (400) and the awsQuery code
/// (`InvalidParameterValue`) are the contract.
pub const INVALID_MAINTENANCE_WINDOW_MESSAGE: &str =
    "Invalid maintenance window format. Should be specified as a range \
     ddd:hh24:mi-ddd:hh24:mi (24H Clock UTC). Example: Sun:23:15-Mon:03:15";

/// Minutes from the start of the week that the `ddd:hh24:mi` point `value`
/// names, or `None` when it is not one.
///
/// The day is matched case-insensitively against [`MAINTENANCE_WINDOW_DAYS`] and
/// the clock fields are exactly two digits each on a 24-hour clock, so neither
/// `notaday:99:99` nor `24:00` is a point.
fn week_minute(value: &str) -> Option<i64> {
    let mut fields = value.split(':');
    let day = fields.next()?;
    let hour = fields.next()?;
    let minute = fields.next()?;
    if fields.next().is_some() {
        return None;
    }
    let day = MAINTENANCE_WINDOW_DAYS
        .iter()
        .position(|candidate| candidate.eq_ignore_ascii_case(day))?;
    let clock_field = |field: &str, max: i64| -> Option<i64> {
        if field.len() != 2 || !field.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        field.parse::<i64>().ok().filter(|value| *value <= max)
    };
    let hour = clock_field(hour, 23)?;
    let minute = clock_field(minute, 59)?;
    Some(i64::try_from(day).unwrap_or(0) * MINUTES_PER_DAY + hour * 60 + minute)
}

/// Why `value` cannot be a `PreferredMaintenanceWindow`, or `None` when it can.
///
/// The rule is the member's own: a range `ddd:hh24:mi-ddd:hh24:mi` on the 24-hour
/// UTC clock whose span is at least [`MAINTENANCE_WINDOW_MIN_MINUTES`]. A window
/// whose end lies before its start wraps to the next week, which is how
/// `sun:23:00-mon:01:30` is a 150-minute window; a window whose two ends are the
/// same point spans nothing and is refused.
///
/// Every operation that accepts a maintenance window goes through this one
/// helper, so a value refused by one is refused by all of them, and the refusal
/// runs before any state is written.
pub fn maintenance_window_refusal(value: &str) -> Option<String> {
    let invalid = || Some(INVALID_MAINTENANCE_WINDOW_MESSAGE.to_string());
    let Some((start, end)) = value.split_once('-') else {
        return invalid();
    };
    let (Some(start), Some(end)) = (week_minute(start), week_minute(end)) else {
        return invalid();
    };
    let span = match end.cmp(&start) {
        ::std::cmp::Ordering::Greater => end - start,
        ::std::cmp::Ordering::Equal => 0,
        ::std::cmp::Ordering::Less => end + MINUTES_PER_WEEK - start,
    };
    if span < MAINTENANCE_WINDOW_MIN_MINUTES {
        return Some(format!(
            "The maintenance window must be at least {MAINTENANCE_WINDOW_MIN_MINUTES} minutes."
        ));
    }
    None
}

/// How long a cluster stays in the status the call that started the work
/// returned (`creating` after a create, `modifying` after an immediate modify,
/// `rebooting cluster nodes` after a reboot) before the clock owes it
/// `available`.
///
/// ElastiCache takes minutes; the emulator chooses a span, and it is
/// deliberately NOT zero, so a caller acting straight after the call still
/// observes the transient status and the state errors that go with it. The due
/// instant is stored on the record and materialised by whichever later call
/// reads it — nothing sleeps.
pub const CLUSTER_TRANSITION: Duration = Duration::from_millis(500);

/// How long a snapshot stays in the status the call that started the work
/// returned — `creating` after a create or a copy before the clock owes it
/// `available`, and `deleting` after a delete before the clock owes its removal.
///
/// The same reasoning as [`CLUSTER_TRANSITION`]: ElastiCache takes minutes, the
/// emulator chooses a span, and it is deliberately NOT zero so a caller acting
/// straight after `CreateSnapshot` or `DeleteSnapshot` still observes the
/// transient status.
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
/// See [`CLUSTER_QUOTA`]; this one bounds the CUSTOM cache parameter groups the
/// store holds, the family defaults of [`default_parameter_groups`] being
/// published by the service rather than created by an account.
pub const PARAMETER_GROUP_QUOTA: usize = 150;
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

/// The longest a `CacheParameterGroupName` may be: the 255-character bound this
/// service publishes for its names ("must contain no more than 255 alphanumeric
/// characters or hyphens"; see [`is_valid_subnet_group_name`]). The member
/// itself documents only that the value "is stored as a lowercase string", so
/// the bound is the one the service's names share.
pub const PARAMETER_GROUP_NAME_MAX_LEN: usize = 255;

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

/// The longest tag key a caller may use: "Maximum key length — 128 Unicode
/// characters in UTF-8", from the tag restrictions ElastiCache publishes
/// ("Tagging your ElastiCache resources", the same restrictions AWS states for
/// tags generally). `Tag$Key` carries no modelled `@length` or `@pattern`, so
/// the boundary validator passes any string and the bound is enforced here.
pub const MAX_TAG_KEY_LEN: usize = 128;

/// The longest tag value a caller may use: "Maximum value length — 256 Unicode
/// characters in UTF-8"; see [`MAX_TAG_KEY_LEN`]. A value of length ZERO is
/// permitted — `Tag$Value` is "the tag's value. May be null".
pub const MAX_TAG_VALUE_LEN: usize = 256;

/// The tag-key prefix AWS reserves for itself: "Do not use the `aws:` prefix in
/// your tag names or values because it is reserved for AWS use. You can't edit
/// or delete tag names or values with this prefix." So a CALLER's tag key may
/// not carry it, in any letter case, wherever a request names tag keys — a
/// create's `Tags`, `AddTagsToResource`'s `Tags`, and
/// `RemoveTagsFromResource`'s `TagKeys`.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Why a request's `Tags` cannot be stored.
#[derive(Debug, Clone)]
pub enum TagRefusal {
    /// A tag a caller may not write, with the rule it breaks: a `Key` the
    /// request omitted or left empty (`Tag$Key` is "the key for the tag. May not
    /// be null" — only the VALUE may be), a key or value past its length bound,
    /// or a key under the reserved [`RESERVED_TAG_KEY_PREFIX`].
    InvalidTag(String),
    /// More tags than [`MAX_TAGS_PER_RESOURCE`].
    TooMany(usize),
}

/// The symbols a tag key or value may carry besides letters, digits and
/// whitespace, from the character set ElastiCache publishes for tags and names
/// in its own refusal ("Tag values may only contain unicode letters, digits,
/// whitespace, or one of these symbols: `_ . : / = + \ - @`"). The
/// letters-and-digits half is UNICODE-wide, which is why
/// [`is_tag_character`] asks [`char::is_alphabetic`] / [`char::is_numeric`]
/// rather than their ASCII-only forms.
pub const TAG_SYMBOLS: [char; 9] = ['_', '.', ':', '/', '=', '+', '\\', '-', '@'];

/// Whether `character` is one a tag key or a tag value may carry; see
/// [`TAG_SYMBOLS`]. Neither `Tag$Key` nor `Tag$Value` carries a modelled
/// `@pattern`, so the boundary validator passes any string and the character set
/// is enforced here — once, for every request that names a tag.
pub fn is_tag_character(character: char) -> bool {
    character.is_alphabetic()
        || character.is_numeric()
        || character.is_whitespace()
        || TAG_SYMBOLS.contains(&character)
}

/// The sentence a charset refusal carries for `member` ("keys" or "values"),
/// in the service's own wording; see [`TAG_SYMBOLS`].
///
/// The published sentence spells the backslash of the symbol list DOUBLED and
/// closes with a stray double quote, so it is reproduced here as the service
/// words it rather than as a tidied transcription: a caller matching the
/// service's text has nothing else to match against.
fn tag_charset_message(member: &str) -> String {
    format!(
        "Tag {member} may only contain unicode letters, digits, whitespace, or one of these \
         symbols: _ . : / = + \\\\ - @\""
    )
}

/// What is wrong with a caller-supplied tag KEY, or `None` when the caller may
/// use it — the one statement of the rule, so a create's `Tags`,
/// `AddTagsToResource` and `RemoveTagsFromResource` cannot disagree about which
/// keys a caller owns.
pub fn tag_key_rejection(key: &str) -> Option<String> {
    // Both bounds of the key's length are the one rule, so the empty key and
    // the key past the published maximum are refused by the same sentence: it
    // states the window a key's length has to fall in.
    if key.is_empty() || key.chars().count() > MAX_TAG_KEY_LEN {
        return Some(format!(
            "Tag keys must be between 1 and {MAX_TAG_KEY_LEN} characters in length."
        ));
    }
    if !key.chars().all(is_tag_character) {
        return Some(tag_charset_message("keys"));
    }
    // Sliced with `get`, which answers `None` rather than panicking when the
    // key's first bytes are the middle of a multi-byte character.
    if key
        .get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
    {
        // The refusal states the RULE — the prefix a caller may not use — and
        // not the key that broke it, which is how the service words it.
        return Some(format!(
            "Tag keys cannot start with the reserved prefix \"{RESERVED_TAG_KEY_PREFIX}\"."
        ));
    }
    None
}

/// What is wrong with a caller-supplied tag VALUE, or `None` when the caller may
/// use it; see [`tag_key_rejection`]. The empty value is permitted — `Tag$Value`
/// "may be null" — and what a non-empty one may contain is the character set
/// [`TAG_SYMBOLS`] states, which the model puts no `@pattern` on.
pub fn tag_value_rejection(value: &str) -> Option<String> {
    // See [`tag_key_rejection`]: the same sentence for the other half of the
    // pair, whose window starts at zero because the empty value is permitted.
    if value.chars().count() > MAX_TAG_VALUE_LEN {
        return Some(format!(
            "Tag values must be between 0 and {MAX_TAG_VALUE_LEN} characters in length."
        ));
    }
    (!value.chars().all(is_tag_character)).then(|| tag_charset_message("values"))
}

/// The tags a request was given as stored pairs, or why they are refused.
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
        .map(|tag| {
            // A tag with no `Value` carries the EMPTY value once stored, so
            // every tag this service reports has one.
            let key = tag.key.unwrap_or_default();
            let value = tag.value.unwrap_or_default();
            match tag_key_rejection(&key).or_else(|| tag_value_rejection(&value)) {
                Some(message) => Err(TagRefusal::InvalidTag(message)),
                None => Ok((key, value)),
            }
        })
        .collect()
}

/// The stored pairs as the modelled `TagList` every tagging operation answers
/// with ("a list of tags as key-value pairs"), in the order the store holds
/// them.
pub fn tag_list(tags: &[(String, String)]) -> Vec<Tag> {
    tags.iter()
        .map(|(key, value)| Tag {
            key: Some(key.clone()),
            value: Some(value.clone()),
        })
        .collect()
}

/// Merge `requested` into `stored`, which is what `AddTagsToResource` does to a
/// resource's tags: a key the resource does not carry is APPENDED, and a key it
/// does carry takes the new value in place, so re-tagging a key adds no tag and
/// leaves the resource's tag order alone. A key the request does not name is
/// untouched.
pub fn merge_tags(stored: &mut Vec<(String, String)>, requested: Vec<(String, String)>) {
    for (key, value) in requested {
        match stored.iter_mut().find(|(held, _)| *held == key) {
            Some(slot) => slot.1 = value,
            None => stored.push((key, value)),
        }
    }
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
/// The `CacheClusterStatus` of a cluster whose nodes `RebootCacheCluster` is
/// rebooting: "during the reboot, the cluster status is set to REBOOTING", and
/// `CacheCluster$CacheClusterStatus`'s own value list spells that status
/// `rebooting cluster nodes`. See [`STATUS_CREATING`].
pub const STATUS_REBOOTING_NODES: &str = "rebooting cluster nodes";
/// The `CacheNode$CacheNodeStatus` of a node being rebooted, from that member's
/// value list ("available, creating, rebooting, or deleting"). Only the nodes
/// `RebootCacheCluster$CacheNodeIdsToReboot` named carry it; the rest keep
/// running.
pub const NODE_STATUS_REBOOTING: &str = "rebooting";
/// See [`STATUS_CREATING`]: the status a cluster carries while a snapshot of it
/// is being taken.
pub const STATUS_SNAPSHOTTING: &str = "snapshotting";
/// The `CacheClusterStatus` of a cluster whose parameter group holds a value its
/// engine cannot run. No operation of this emulator puts a cluster in it — the
/// parameter writers refuse such a value outright — but it is one of the states
/// `RebootCacheCluster` accepts ([`REBOOTABLE_CLUSTER_STATES`]), so the refusal
/// that names them names it too.
pub const STATUS_INCOMPATIBLE_PARAMETERS: &str = "incompatible-parameters";

/// The `CacheClusterStatus` values `RebootCacheCluster` accepts.
///
/// A reboot is not gated on `available` alone: the service states the states it
/// accepts in the refusal itself (see [`reboot_state_refusal`]), and a cluster
/// whose parameters the engine rejected or that is being snapshotted can still be
/// rebooted — which is how a client recovers one.
pub const REBOOTABLE_CLUSTER_STATES: [&str; 3] = [
    STATUS_AVAILABLE,
    STATUS_INCOMPATIBLE_PARAMETERS,
    STATUS_SNAPSHOTTING,
];

/// How ElastiCache refuses a reboot of a cluster that is in none of
/// [`REBOOTABLE_CLUSTER_STATES`]: the refusal states the rule — the states a
/// reboot IS accepted in — and then the cluster and the state it is actually in.
pub fn reboot_state_refusal(cache_cluster_id: &str, status: &str) -> String {
    format!(
        "Can only reboot cache clusters with state in: {}. Cluster {cache_cluster_id} has state: \
         {status}.",
        REBOOTABLE_CLUSTER_STATES.join(", ")
    )
}

/// How ElastiCache refuses a MODIFICATION of a replication group that is not
/// `available`: the message states the requirement — the state the group has to
/// be in and the action it gates — and names neither the group nor the state it
/// is currently in.
///
/// One statement, so the operations that change a group's configuration (the two
/// replica-count operations and `ModifyReplicationGroup`) cannot word their
/// `InvalidReplicationGroupStateFault` differently from one another.
pub fn replication_group_modify_state_refusal() -> String {
    format!("Replication group must be in {STATUS_AVAILABLE} state to modify.")
}

/// How ElastiCache refuses a replica-count change that asks for the count a
/// shard already has: the `NoOperationFault` states that the requested count
/// already matches, naming neither the shard nor the group.
pub const REPLICA_COUNT_UNCHANGED_MESSAGE: &str =
    "Requested new replica count already matches current number of replicas.";

/// The `SourceType` values this service records an event under, from the
/// model's `SourceType` enum (`smithy_query(query="shape",shape="SourceType")`:
/// `cache-cluster | cache-parameter-group | cache-security-group |
/// cache-subnet-group | replication-group | serverless-cache |
/// serverless-cache-snapshot | user | user-group`).
///
/// Only the resources this service both persists and changes can be the origin
/// of an event; the rest belong to operations that are still scaffolded 501s
/// and will record their own events when they land, and a published catalog
/// (service updates, reserved-cache-node offerings, the engines' default
/// parameter groups) is never changed by a call, so nothing happens to it to
/// record. A snapshot of a cluster is reported under the CLUSTER it was taken
/// from, which is the source the event happened to — the enum has no member
/// for a cache-cluster snapshot.
pub const SOURCE_TYPE_CACHE_CLUSTER: &str = "cache-cluster";
/// See [`SOURCE_TYPE_CACHE_CLUSTER`].
pub const SOURCE_TYPE_CACHE_SUBNET_GROUP: &str = "cache-subnet-group";
/// See [`SOURCE_TYPE_CACHE_CLUSTER`].
pub const SOURCE_TYPE_SERVERLESS_CACHE: &str = "serverless-cache";
/// See [`SOURCE_TYPE_CACHE_CLUSTER`]. Unlike a cluster's snapshot, a serverless
/// cache's snapshot IS an event source of its own in the enum, so what happens
/// to one is recorded under the snapshot rather than under the cache it was
/// taken from.
pub const SOURCE_TYPE_SERVERLESS_CACHE_SNAPSHOT: &str = "serverless-cache-snapshot";
/// See [`SOURCE_TYPE_CACHE_CLUSTER`].
pub const SOURCE_TYPE_USER: &str = "user";
/// See [`SOURCE_TYPE_CACHE_CLUSTER`].
pub const SOURCE_TYPE_REPLICATION_GROUP: &str = "replication-group";
/// See [`SOURCE_TYPE_CACHE_CLUSTER`].
pub const SOURCE_TYPE_CACHE_PARAMETER_GROUP: &str = "cache-parameter-group";

/// The window `DescribeEvents` reports when the request bounds neither end of
/// it, as the operation documents: "By default, only the events occurring
/// within the last hour are returned."
pub const DEFAULT_EVENT_WINDOW: Duration = Duration::from_secs(60 * 60);

/// How long a recorded event is kept, and so the furthest back
/// `DescribeEvents` can look, as the operation documents: "you can retrieve up
/// to 14 days' worth of events if necessary."
pub const EVENT_RETENTION: Duration = Duration::from_secs(14 * 24 * 60 * 60);

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

/// The stored snapshot source a `DescribeSnapshots$SnapshotSource` value
/// selects, or the reason it names no creation source.
///
/// `SnapshotSource` is a plain string with NO modelled enum or pattern, so the
/// vocabulary is enforced here, and it is a SUPERSET of the one the response
/// emits: each source is named either by the filter's spelling the member
/// documents ([`SNAPSHOT_SOURCE_FILTER_SYSTEM`] / [`SNAPSHOT_SOURCE_FILTER_USER`])
/// or by the spelling `Snapshot$SnapshotSource` reports it under
/// ([`SNAPSHOT_SOURCE_AUTOMATED`] / [`SNAPSHOT_SOURCE_MANUAL`]). That is what
/// makes "read a snapshot's `SnapshotSource`, then list only the snapshots of
/// that source" a round trip rather than a refusal. Matched ignoring ASCII
/// case, as this service's other unconstrained filter vocabularies are (see
/// [`canonical_offering_type`]): a creation source is a vocabulary word, not an
/// identity.
pub fn snapshot_source_filter(value: &str) -> Result<&'static str, String> {
    for (filter_spelling, reported_spelling) in [
        (SNAPSHOT_SOURCE_FILTER_SYSTEM, SNAPSHOT_SOURCE_AUTOMATED),
        (SNAPSHOT_SOURCE_FILTER_USER, SNAPSHOT_SOURCE_MANUAL),
    ] {
        if value.eq_ignore_ascii_case(filter_spelling)
            || value.eq_ignore_ascii_case(reported_spelling)
        {
            return Ok(reported_spelling);
        }
    }
    Err(format!(
        "Invalid value {value} for SnapshotSource: must be one of \
         {SNAPSHOT_SOURCE_FILTER_SYSTEM} | {SNAPSHOT_SOURCE_AUTOMATED} | \
         {SNAPSHOT_SOURCE_FILTER_USER} | {SNAPSHOT_SOURCE_MANUAL}."
    ))
}

/// The node type ElastiCache cannot snapshot a Valkey / Redis OSS cluster on,
/// from `SnapshotFeatureNotSupportedFault`: "Creating a snapshot of a Valkey or
/// Redis OSS cluster running on a `cache.t1.micro` cache node" is not
/// supported.
pub const UNSNAPSHOTTABLE_NODE_TYPE: &str = "cache.t1.micro";

/// The `ParameterApplyStatus` / `CacheNode$ParameterGroupStatus` of a cache
/// node that runs the current values of the parameter group its cluster is
/// associated with.
pub const PARAMETER_APPLY_STATUS: &str = "in-sync";

/// The `ParameterApplyStatus` / `CacheNode$ParameterGroupStatus` of a cache
/// node whose group has had a parameter the engine applies only at a restart
/// changed on it: the value is SET on the group, and the node keeps running the
/// one it was started with until it is rebooted.
///
/// `Parameter$ChangeType` is what says which parameters those are
/// ([`CHANGE_TYPE_REQUIRES_REBOOT`]), and `RebootCacheCluster` — which "applies
/// any modified cache parameter groups to the cluster" — is what ends it.
pub const PARAMETER_APPLY_STATUS_PENDING_REBOOT: &str = "pending-reboot";

/// The `SecurityGroupMembership$Status` of a VPC security group a cluster was
/// created with. The emulator models no EC2 security-group lifecycle, so a
/// membership is reported in the one state a created cluster's membership has.
pub const SECURITY_GROUP_STATUS: &str = "active";

/// The `NotificationConfiguration$TopicStatus` of the SNS topic a cluster was
/// given. The emulator models no SNS subscription lifecycle, so a configured
/// topic is reported in the one state a configured topic has.
pub const NOTIFICATION_TOPIC_STATUS: &str = "active";

/// `NotificationTopicStatus`'s two documented values: "The status of the Amazon
/// SNS notification topic … Notifications are sent only if the status is active.
/// Valid values: active | inactive." The member is a plain `String`, so these
/// are the spellings a request is held to; [`NOTIFICATION_TOPIC_STATUS`] is the
/// state a CONFIGURED topic is reported in, which is the `active` one.
pub const NOTIFICATION_STATUS_ACTIVE: &str = NOTIFICATION_TOPIC_STATUS;
/// See [`NOTIFICATION_STATUS_ACTIVE`].
pub const NOTIFICATION_STATUS_INACTIVE: &str = "inactive";

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

/// The name suffix of the CLUSTER-MODE-ON default parameter group of a family:
/// `default.<family>.cluster.on`, the group `CreateReplicationGroup`'s own
/// documentation names ("To create a Valkey or Redis OSS (cluster mode enabled)
/// replication group, use CacheParameterGroupName=default.redis3.2.cluster.on").
pub const CLUSTER_MODE_ON_SUFFIX: &str = ".cluster.on";

/// The parameter whose value IS cluster mode, and the value that turns it on.
///
/// A family publishes it from the engine version that introduced cluster mode
/// onward, which is what makes the parameter the one statement of which families
/// have a cluster-mode-on default group — see [`family_publishes_cluster_mode`].
pub const CLUSTER_ENABLED_PARAMETER: &str = "cluster-enabled";
/// See [`CLUSTER_ENABLED_PARAMETER`].
pub const CLUSTER_ENABLED_ON: &str = "yes";

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

/// True when `family` publishes the cluster-mode parameter, and so has a
/// cluster-mode-on default group beside its plain one.
///
/// Read off the family's OWN parameter catalog rather than from a second list of
/// family names: a family carries [`CLUSTER_ENABLED_PARAMETER`] exactly when its
/// line covers the engine version that introduced cluster mode, so the groups
/// this service holds and the parameters they report cannot disagree about which
/// families can run cluster mode.
pub fn family_publishes_cluster_mode(family: &str) -> bool {
    family_default_parameters(family).is_some_and(|parameters| {
        parameters
            .values()
            .any(|parameter| parameter.name == CLUSTER_ENABLED_PARAMETER)
    })
}

/// The default parameter groups an account holds the moment ElastiCache becomes
/// available in it: one per published family, plus the CLUSTER-MODE-ON default
/// of every family that can run cluster mode — which is why
/// `DescribeCacheParameterGroups` answers with groups before anything has been
/// created. Seeded into
/// [`ComAmazonawsElasticacheAmazonelasticachev9State::cache_parameter_groups`]
/// when the store is created, so the groups a read reports and the
/// [`default_parameter_group_name`] a `CreateCacheCluster` applies cannot
/// disagree.
///
/// A family's two defaults differ by ONE value: the cluster-mode-on group holds
/// [`CLUSTER_ENABLED_ON`] for [`CLUSTER_ENABLED_PARAMETER`] and takes the
/// family's engine defaults for everything else, which is what makes it a
/// customised copy of the plain default rather than a second catalog.
pub fn default_parameter_groups() -> BTreeMap<String, CacheParameterGroupRecord> {
    let mut groups = BTreeMap::new();
    for family in PARAMETER_GROUP_FAMILIES {
        let plain = CacheParameterGroupRecord {
            name: format!("{DEFAULT_PARAMETER_GROUP_PREFIX}{family}"),
            family: family.to_string(),
            // The description AWS gives a family's own default group.
            description: format!("Default parameter group for {family}"),
            // Nothing tags a group the service itself provides.
            tags: Vec::new(),
            // A family default starts out being exactly its family's engine
            // defaults: nothing has set a value on it.
            parameter_values: BTreeMap::new(),
        };
        if family_publishes_cluster_mode(family) {
            let cluster_on = CacheParameterGroupRecord {
                name: format!("{}{CLUSTER_MODE_ON_SUFFIX}", plain.name),
                family: family.to_string(),
                description: format!(
                    "Customized default parameter group for {family} with cluster mode on"
                ),
                tags: Vec::new(),
                parameter_values: BTreeMap::from([(
                    CLUSTER_ENABLED_PARAMETER.to_string(),
                    CLUSTER_ENABLED_ON.to_string(),
                )]),
            };
            groups.insert(cluster_on.name.clone(), cluster_on);
        }
        groups.insert(plain.name.clone(), plain);
    }
    groups
}

/// One cache parameter group, as this emulator persists it: a family default
/// the store comes up holding, or a custom one `CreateCacheParameterGroup`
/// created.
///
/// Every group is one an account holds in the Region it is read from — a family
/// default is provided in every Region, and a custom one is created in the
/// Region of the create — so the ARN a read reports names the Region of the
/// request reading it; see [`CacheParameterGroupRecord::to_modeled`].
#[derive(Debug, Clone)]
pub struct CacheParameterGroupRecord {
    /// The group's identity, which "is stored as a lowercase string"
    /// (`CreateCacheParameterGroup$CacheParameterGroupName`).
    pub name: String,
    /// One of [`PARAMETER_GROUP_FAMILIES`] (`CacheParameterGroupFamily`).
    pub family: String,
    /// `CacheParameterGroup$Description`.
    pub description: String,
    /// The tags `CreateCacheParameterGroup` was given; see
    /// [`CacheClusterRecord::tags`].
    pub tags: Vec<(String, String)>,
    /// The value SET on this group for a parameter, keyed by
    /// `Parameter$ParameterName`.
    ///
    /// A group holds only the values it DIFFERS from its family's engine
    /// defaults by, never a copy of the catalog: that is what makes a group's
    /// effective parameter list "the family defaults with these laid over
    /// them", and what makes a parameter's `Parameter$Source` derivable — a
    /// name in this map is the customer's value ([`PARAMETER_SOURCE_USER`]),
    /// one that is not is the service's ([`PARAMETER_SOURCE_SYSTEM`]).
    /// `ModifyCacheParameterGroup` sets an entry and
    /// `ResetCacheParameterGroup` removes one, which puts the parameter back to
    /// the engine's own default.
    pub parameter_values: BTreeMap<String, String>,
}

impl CacheParameterGroupRecord {
    /// The value set on this group for the parameter `name`, or `None` when
    /// this group reports the engine's own default for it; see
    /// [`Self::parameter_values`].
    pub fn set_value(&self, name: &str) -> Option<&str> {
        self.parameter_values.get(name).map(String::as_str)
    }

    /// True when this group turns cluster mode ON: the value it holds for
    /// [`CLUSTER_ENABLED_PARAMETER`] is [`CLUSTER_ENABLED_ON`].
    ///
    /// Read off the group's own parameter value rather than off its NAME, so a
    /// group's reported parameters and what naming it does cannot disagree.
    pub fn cluster_mode_on(&self) -> bool {
        self.set_value(CLUSTER_ENABLED_PARAMETER) == Some(CLUSTER_ENABLED_ON)
    }

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

/// The `Parameter$Source` of a value that is the engine's own default rather
/// than one a customer set.
///
/// `DescribeCacheParameters$Source` documents the sources a parameter is
/// reported under ("The parameter types to return. Valid values: user | system |
/// engine-default"); an engine default is the service's own value, so it is
/// reported as `system`.
pub const PARAMETER_SOURCE_SYSTEM: &str = "system";

/// The `Parameter$Source` of a value a customer set on the group being read,
/// from the same `DescribeCacheParameters$Source` vocabulary as
/// [`PARAMETER_SOURCE_SYSTEM`].
pub const PARAMETER_SOURCE_USER: &str = "user";

/// The third spelling `DescribeCacheParameters$Source` accepts as a FILTER
/// ("Valid values: user | system | engine-default").
///
/// It selects the same parameters as [`PARAMETER_SOURCE_SYSTEM`] — the ones a
/// group reports the engine's own default for — because an engine default IS
/// the service's value and is reported under the `system` spelling; see
/// [`PARAMETER_SOURCE_SYSTEM`].
pub const PARAMETER_SOURCE_FILTER_ENGINE_DEFAULT: &str = "engine-default";

/// The `Parameter$Source` the `DescribeCacheParameters$Source` filter `value`
/// selects, or the reason that value is refused.
///
/// `Source` is a plain string with NO modelled enum or pattern, so the
/// vocabulary its own documentation states ("Valid values: user | system |
/// engine-default") is enforced here, and it is a SUPERSET of the two spellings
/// a response reports: `engine-default` and `system` both select the parameters
/// whose value is the engine's own. Matched ignoring ASCII case, as this
/// service's other unconstrained filter vocabularies are (see
/// [`snapshot_source_filter`]): a source is a vocabulary word, not an identity.
pub fn parameter_source_filter(value: &str) -> Result<&'static str, String> {
    for (spelling, selected) in [
        (PARAMETER_SOURCE_USER, PARAMETER_SOURCE_USER),
        (PARAMETER_SOURCE_SYSTEM, PARAMETER_SOURCE_SYSTEM),
        (
            PARAMETER_SOURCE_FILTER_ENGINE_DEFAULT,
            PARAMETER_SOURCE_SYSTEM,
        ),
    ] {
        if value.eq_ignore_ascii_case(spelling) {
            return Ok(selected);
        }
    }
    Err(format!(
        "Invalid value {value} for Source: must be one of {PARAMETER_SOURCE_USER} | \
         {PARAMETER_SOURCE_SYSTEM} | {PARAMETER_SOURCE_FILTER_ENGINE_DEFAULT}."
    ))
}

/// `ChangeType`'s values (`smithy_query(query="shape", shape="ChangeType")`:
/// `immediate | requires-reboot`): whether a change to the parameter is applied
/// at once or only once the nodes are rebooted.
pub const CHANGE_TYPE_IMMEDIATE: &str = "immediate";
/// See [`CHANGE_TYPE_IMMEDIATE`].
pub const CHANGE_TYPE_REQUIRES_REBOOT: &str = "requires-reboot";

/// The two parameter catalogs this service publishes, one per cache engine.
///
/// Valkey is the Redis OSS-compatible engine, so the Valkey families draw on the
/// same catalog as the Redis OSS ones; Memcached has a catalog of its own.
const PARAMETER_ENGINE_MEMCACHED: &str = "memcached";
/// See [`PARAMETER_ENGINE_MEMCACHED`].
const PARAMETER_ENGINE_REDIS: &str = "redis";

/// The minor version of a family line that covers every minor of its major
/// (`redis7` covers every Redis OSS 7.x), so every parameter that major
/// introduced belongs to it.
const ANY_MINOR: i32 = i32::MAX;

/// Each published family's parameter catalog and the version its parameters are
/// selected by.
///
/// A family's parameters are its ENGINE's, which is why each row names the
/// catalog the family draws on. The version is the HIGHEST engine version the
/// family's line covers, because a family carries every parameter any version on
/// its line introduced: `redis6.x` covers Redis OSS 6.0 through 6.2, and
/// `redis7` / `valkey7` / `valkey8` cover every minor of their major — see
/// [`ANY_MINOR`].
///
/// The family names are [`PARAMETER_GROUP_FAMILIES`], this service's published
/// set; this table says what each of them MEANS to the parameter catalog.
pub const FAMILY_LINES: [(&str, &str, (i32, i32)); PARAMETER_GROUP_FAMILIES.len()] = [
    ("memcached1.4", PARAMETER_ENGINE_MEMCACHED, (1, 4)),
    ("memcached1.5", PARAMETER_ENGINE_MEMCACHED, (1, 5)),
    ("memcached1.6", PARAMETER_ENGINE_MEMCACHED, (1, ANY_MINOR)),
    ("redis2.6", PARAMETER_ENGINE_REDIS, (2, 6)),
    ("redis2.8", PARAMETER_ENGINE_REDIS, (2, 8)),
    ("redis3.2", PARAMETER_ENGINE_REDIS, (3, 2)),
    ("redis4.0", PARAMETER_ENGINE_REDIS, (4, ANY_MINOR)),
    ("redis5.0", PARAMETER_ENGINE_REDIS, (5, ANY_MINOR)),
    ("redis6.x", PARAMETER_ENGINE_REDIS, (6, ANY_MINOR)),
    ("redis7", PARAMETER_ENGINE_REDIS, (7, ANY_MINOR)),
    ("valkey7", PARAMETER_ENGINE_REDIS, (7, ANY_MINOR)),
    ("valkey8", PARAMETER_ENGINE_REDIS, (8, ANY_MINOR)),
];

/// The parameter catalog `family` draws on and the version its parameters are
/// selected by, or `None` when no published family carries that name.
pub fn family_line(family: &str) -> Option<(&'static str, (i32, i32))> {
    FAMILY_LINES
        .iter()
        .find(|(name, _, _)| *name == family)
        .map(|(_, engine, version)| (*engine, *version))
}

/// `version` as the `(major, minor)` pair a family line is compared against, so
/// a parameter's `MinimumEngineVersion` ("2.6.13", "6.2.6") and a family's line
/// are ordered by the same rule. A part that is not a number counts as 0, which
/// keeps the comparison total.
fn version_rank(version: &str) -> (i32, i32) {
    let mut parts = version.split('.');
    let major = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    (major, minor)
}

/// One row of an engine's parameter catalog, in column order: the parameter's
/// name, the engine's default value for it, its description, its data type, the
/// values it may take, whether a customer may modify it, the earliest engine
/// version it applies to, whether a change to it is immediate or needs a reboot,
/// and whether its value depends on the cache node type.
///
/// The allowed-values column is the `Parameter$AllowedValues` the catalog
/// publishes, in the three forms ElastiCache writes it in: an enumeration of the
/// literal values (`yes,no`), a numeric RANGE as `<lo>-<hi>` where the engine
/// bounds the value at both ends and `<lo>-` where it bounds only the bottom
/// (optionally with the discrete values that sit outside the range in front of
/// it, `0,20-`), and the EMPTY string only where a customer cannot set the
/// parameter at all — a value the engine or the node type fixes
/// (`IsModifiable: false`), which is the one case there is no set to discover.
/// So every MODIFIABLE numeric parameter publishes its range: a client that
/// reads the catalog to decide what it may set finds the bounds to validate
/// against.
struct ParameterRow(
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    bool,
    &'static str,
    &'static str,
    bool,
);

/// The parameters ElastiCache publishes for its Redis OSS-compatible engines
/// (Redis OSS and Valkey).
///
/// Source: the parameter tables of the ElastiCache User Guide's "Redis-specific
/// parameters" / "Valkey-specific parameters" sections — the names, data types,
/// value sets, modifiability, change types and minimum engine versions below are
/// that published catalog, and the engine's own defaults are the values. Nothing
/// in the Smithy model enumerates parameters (`Parameter` describes only the
/// shape of one), so this table is the emulator's single statement of what the
/// Redis OSS-compatible engine defaults ARE; it is deliberately a representative
/// catalog rather than every parameter AWS ships, and every family that draws on
/// it reports exactly the rows whose minimum engine version its line covers.
///
/// The allowed-values column follows [`ParameterRow`]: each numeric parameter a
/// customer may modify carries the range the ENGINE constrains it to, as the
/// Redis OSS / Valkey configuration reference bounds it (for example active
/// defragmentation's effort is a percentage, `maxmemory-samples` is 1 to 64, a
/// replication backlog is at least 16 KiB, and a protocol element is 1 MiB to
/// 512 MiB), so a client scanning the catalog for what it may tune finds both
/// bounds wherever the engine publishes both.
const REDIS_PARAMETERS: &[ParameterRow] = &[
    ParameterRow(
        "acl-pubsub-default",
        "resetchannels",
        "The default pub/sub channel permissions a newly created user is given.",
        "string",
        "allchannels,resetchannels",
        true,
        "6.2.6",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "active-defrag-cycle-max",
        "75",
        "The largest share of CPU, as a percentage, active defragmentation may use.",
        "integer",
        "1-75",
        true,
        "4.0.9",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "active-defrag-cycle-min",
        "5",
        "The smallest share of CPU, as a percentage, active defragmentation uses once it \
         starts.",
        "integer",
        "1-75",
        true,
        "4.0.9",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "active-defrag-ignore-bytes",
        "104857600",
        "How much fragmentation, in bytes, is tolerated before active defragmentation starts.",
        "integer",
        "1048576-",
        true,
        "4.0.9",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "active-defrag-max-scan-fields",
        "1000",
        "The largest number of fields of a hash, set, sorted set or list active \
         defragmentation processes in one scan.",
        "integer",
        "1-1000000",
        true,
        "4.0.9",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "active-defrag-threshold-lower",
        "10",
        "The fragmentation, as a percentage, at which active defragmentation starts.",
        "integer",
        "1-100",
        true,
        "4.0.9",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "active-defrag-threshold-upper",
        "100",
        "The fragmentation, as a percentage, at which active defragmentation uses its \
         largest share of CPU.",
        "integer",
        "1-100",
        true,
        "4.0.9",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "active-expire-effort",
        "1",
        "How much effort the node spends expiring keys, from the least to the most eager.",
        "integer",
        "1-10",
        true,
        "7.0.0",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "activedefrag",
        "no",
        "Whether active memory defragmentation is enabled.",
        "string",
        "yes,no",
        true,
        "4.0.9",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "activerehashing",
        "yes",
        "Whether the main dictionary is actively rehashed.",
        "string",
        "yes,no",
        false,
        "2.6.13",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "client-output-buffer-limit-pubsub-hard-limit",
        "33554432",
        "The output buffer size, in bytes, a pub/sub client is disconnected at.",
        "integer",
        "0-",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "client-output-buffer-limit-pubsub-soft-limit",
        "8388608",
        "The output buffer size, in bytes, a pub/sub client may stay above only for \
         client-output-buffer-limit-pubsub-soft-seconds.",
        "integer",
        "0-",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "client-output-buffer-limit-pubsub-soft-seconds",
        "60",
        "How long, in seconds, a pub/sub client may stay above the soft limit.",
        "integer",
        "0-",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "cluster-enabled",
        "no",
        "Whether cluster mode is enabled for the cluster.",
        "string",
        "yes,no",
        false,
        "3.2.4",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "databases",
        "16",
        "The number of logical databases a node holds.",
        "integer",
        "1-",
        false,
        "2.6.13",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "hash-max-listpack-entries",
        "128",
        "The largest hash, in entries, kept in the memory-efficient listpack encoding.",
        "integer",
        "0-",
        true,
        "7.0.0",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "hash-max-ziplist-entries",
        "512",
        "The largest hash, in entries, kept in the memory-efficient ziplist encoding.",
        "integer",
        "0-",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "hash-max-ziplist-value",
        "64",
        "The largest hash value, in bytes, kept in the memory-efficient ziplist encoding.",
        "integer",
        "0-",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "hll-sparse-max-bytes",
        "3000",
        "The largest HyperLogLog, in bytes, kept in the memory-efficient sparse \
         representation.",
        "integer",
        "1-16000",
        true,
        "2.8.6",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "lazyfree-lazy-eviction",
        "no",
        "Whether an evicted key is freed in a background thread.",
        "string",
        "yes,no",
        true,
        "4.0.9",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "lazyfree-lazy-expire",
        "no",
        "Whether an expired key is freed in a background thread.",
        "string",
        "yes,no",
        true,
        "4.0.9",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "list-max-ziplist-entries",
        "128",
        "The largest list, in entries, kept in the memory-efficient ziplist encoding.",
        "integer",
        "0-",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "lua-time-limit",
        "5000",
        "How long, in milliseconds, a Lua script may run before the node starts answering \
         other clients with BUSY.",
        "integer",
        "",
        false,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "maxclients",
        "65000",
        "The maximum number of client connections a node accepts.",
        "integer",
        "",
        false,
        "2.6.13",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "maxmemory-policy",
        "volatile-lru",
        "The eviction policy applied once the data stored reaches maxmemory.",
        "string",
        "volatile-lru,allkeys-lru,volatile-lfu,allkeys-lfu,volatile-random,allkeys-random,\
         volatile-ttl,noeviction",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "maxmemory-samples",
        "3",
        "How many keys an approximated eviction policy samples before it picks one.",
        "integer",
        "1-64",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "min-replicas-max-lag",
        "10",
        "How long, in seconds, a replica may lag before it stops counting towards \
         min-replicas-to-write.",
        "integer",
        "0-",
        true,
        "5.0.0",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "min-replicas-to-write",
        "0",
        "How many connected replicas the primary needs before it accepts writes.",
        "integer",
        "0-",
        true,
        "5.0.0",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "notify-keyspace-events",
        "",
        "The classes of keyspace event published to pub/sub.",
        "string",
        "",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "proto-max-bulk-len",
        "536870912",
        "The largest single protocol element, in bytes, a request may carry.",
        "integer",
        "1048576-536870912",
        true,
        "5.0.0",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "repl-backlog-size",
        "1048576",
        "The size, in bytes, of the replication backlog a disconnected replica resynchronises \
         from.",
        "integer",
        "16384-",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "repl-backlog-ttl",
        "3600",
        "How long, in seconds, the primary keeps the replication backlog after the last \
         replica disconnects.",
        "integer",
        "0-",
        true,
        "2.8.6",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "reserved-memory-percent",
        "25",
        "The percentage of a node's memory reserved for non-data use.",
        "integer",
        "0-100",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "set-max-intset-entries",
        "512",
        "The largest set of integers, in entries, kept in the memory-efficient intset \
         encoding.",
        "integer",
        "0-",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "slowlog-log-slower-than",
        "10000",
        "How long, in microseconds, a command must take to be recorded in the slow log.",
        "integer",
        "0-",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "slowlog-max-len",
        "128",
        "How many entries the slow log keeps.",
        "integer",
        "0-",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "stream-node-max-bytes",
        "4096",
        "The largest stream macro node, in bytes.",
        "integer",
        "0-",
        true,
        "5.0.0",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "stream-node-max-entries",
        "100",
        "The largest stream macro node, in entries.",
        "integer",
        "0-",
        true,
        "5.0.0",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "tcp-keepalive",
        "300",
        "How often, in seconds, a TCP ACK is sent to an idle client, or 0 to send none.",
        "integer",
        "0-",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "timeout",
        "0",
        "How long, in seconds, a client may stay idle before it is closed, or 0 to never \
         close one.",
        "integer",
        "0,20-",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "zset-max-ziplist-entries",
        "128",
        "The largest sorted set, in entries, kept in the memory-efficient ziplist encoding.",
        "integer",
        "0-",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "zset-max-ziplist-value",
        "64",
        "The largest sorted set value, in bytes, kept in the memory-efficient ziplist \
         encoding.",
        "integer",
        "0-",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "maxmemory",
        "",
        "The memory, in bytes, available to the data stored on a node.",
        "integer",
        "",
        false,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        // The node's own capacity, so the value is per cache node type.
        true,
    ),
];

/// The parameters ElastiCache publishes for its Memcached engine.
///
/// Source: the parameter tables of the ElastiCache User Guide's "Memcached
/// specific parameters" section, read the same way as [`REDIS_PARAMETERS`].
const MEMCACHED_PARAMETERS: &[ParameterRow] = &[
    ParameterRow(
        "backlog_queue_limit",
        "1024",
        "How many connections may wait in the backlog queue.",
        "integer",
        "",
        false,
        "1.4.5",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "binding_protocol",
        "auto",
        "The protocol a client connection is read as.",
        "string",
        "auto,ascii,binary",
        true,
        "1.4.5",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "cas_disabled",
        "0",
        "Whether check-and-set (CAS) operations are disabled.",
        "boolean",
        "0,1",
        true,
        "1.4.5",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "chunk_size",
        "48",
        "The smallest chunk size, in bytes, a key-value pair is stored in.",
        "integer",
        "1-",
        true,
        "1.4.5",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "chunk_size_growth_factor",
        "1.25",
        "The growth factor between the chunk sizes of consecutive slab classes.",
        "float",
        "1.0-",
        true,
        "1.4.5",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "disable_flush_all",
        "0",
        "Whether the flush_all command is refused.",
        "boolean",
        "0,1",
        true,
        "1.4.14",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "error_on_memory_exhausted",
        "1",
        "Whether a store with no memory left is answered with an error instead of evicting.",
        "boolean",
        "0,1",
        true,
        "1.4.14",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "hashpower_init",
        "0",
        "The initial size of the hash table, as a power of two.",
        "integer",
        "0-",
        true,
        "1.4.14",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "large_memory_pages",
        "0",
        "Whether large memory pages are used.",
        "boolean",
        "0,1",
        false,
        "1.4.5",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "lock_down_paged_memory",
        "0",
        "Whether paged memory is locked down.",
        "boolean",
        "0,1",
        false,
        "1.4.5",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "max_item_size",
        "1048576",
        "The largest item, in bytes, that may be stored.",
        "integer",
        "1024-",
        true,
        "1.4.14",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "maxconns_fast",
        "0",
        "Whether a connection over the limit is closed immediately rather than queued.",
        "boolean",
        "0,1",
        true,
        "1.4.14",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "memcached_connections_overhead",
        "100",
        "The memory, in megabytes, reserved for connection overhead.",
        "integer",
        "0-",
        true,
        "1.4.5",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "requests_per_event",
        "20",
        "How many requests one connection is served per event before the next is served.",
        "integer",
        "1-",
        true,
        "1.4.5",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "slab_automove",
        "1",
        "How a slab page is moved between slab classes.",
        "integer",
        "0,1,2",
        true,
        "1.4.5",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "slab_chunk_max",
        "524288",
        "The largest chunk size, in bytes, a slab page is divided into.",
        "integer",
        "1-",
        true,
        "1.5.10",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "slab_reassign",
        "1",
        "Whether a slab page may be reassigned between slab classes.",
        "boolean",
        "0,1",
        true,
        "1.4.5",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "max_cache_memory",
        "",
        "The memory, in megabytes, available to the cache on a node.",
        "integer",
        "",
        false,
        "1.4.5",
        CHANGE_TYPE_REQUIRES_REBOOT,
        // The node's own capacity, so the value is per cache node type.
        true,
    ),
    ParameterRow(
        "num_threads",
        "",
        "How many threads a node serves requests with.",
        "integer",
        "",
        false,
        "1.4.5",
        CHANGE_TYPE_REQUIRES_REBOOT,
        // One per vCPU of the node type, so the value is per cache node type.
        true,
    ),
];

/// The memory, in mebibytes, and the vCPU count the emulator attributes to each
/// cache node SIZE — the last segment of a `cache.<family>.<size>` node type.
///
/// Like [`OFFERING_NODE_TYPES`]' hourly rates these are the EMULATOR's own
/// scale, not a claim about AWS's published node capacities: AWS publishes those
/// outside this API and the values below are derived per node size rather than
/// copied from it. They DO reach the wire — `DescribeEngineDefaultParameters`
/// reports `maxmemory` / `max_cache_memory` / `num_threads` per cache node type
/// from them — so read them as this emulator's approximations of those
/// capacities, not as AWS's numbers. They rise with the
/// node size, so the node-type-specific values this catalog reports keep the
/// ordering a caller picks a node type for, and they are fixed, so every read
/// reports the same ones.
const NODE_SIZES: [(&str, i64, i64); 7] = [
    ("micro", 512, 2),
    ("small", 1_024, 2),
    ("medium", 3_072, 2),
    ("large", 6_144, 2),
    ("xlarge", 12_288, 4),
    ("2xlarge", 24_576, 8),
    ("4xlarge", 49_152, 16),
];

/// The memory (mebibytes) and vCPU count of `cache_node_type`, or `None` when
/// its size is not one [`NODE_SIZES`] carries.
fn node_type_capacity(cache_node_type: &str) -> Option<(i64, i64)> {
    let size = cache_node_type.split('.').next_back()?;
    NODE_SIZES
        .iter()
        .find(|(name, _, _)| *name == size)
        .map(|(_, memory_mib, vcpus)| (*memory_mib, *vcpus))
}

/// The value the parameter `name` takes on a node type of this capacity, or
/// `None` when that parameter's value does not depend on the node type.
///
/// This is the one place a node-type-specific parameter's value is computed, so
/// the two catalogs cannot derive it differently.
fn node_type_specific_value(name: &str, memory_mib: i64, vcpus: i64) -> Option<String> {
    /// Mebibytes to bytes.
    const BYTES_PER_MIB: i64 = 1024 * 1024;
    match name {
        // The data a Redis OSS / Valkey node may hold, in bytes.
        "maxmemory" => Some((memory_mib * BYTES_PER_MIB).to_string()),
        // The cache a Memcached node may hold, in megabytes.
        "max_cache_memory" => Some(memory_mib.to_string()),
        // One Memcached worker thread per vCPU.
        "num_threads" => Some(vcpus.to_string()),
        _ => None,
    }
}

/// One engine default parameter of one cache parameter group family, as this
/// emulator persists it.
///
/// A parameter whose value depends on the cache node type carries that value per
/// node type in `node_type_values` and no single `value`, which is what makes it
/// a `CacheNodeTypeSpecificParameter` rather than a `Parameter` on the wire.
#[derive(Debug, Clone)]
pub struct ParameterRecord {
    /// `Parameter$ParameterName`.
    pub name: String,
    /// `Parameter$ParameterValue` — the engine's default for it, `None` for a
    /// parameter whose value is per cache node type.
    pub value: Option<String>,
    /// `Parameter$Description`.
    pub description: String,
    /// `Parameter$DataType`.
    pub data_type: String,
    /// `Parameter$AllowedValues`, absent where the engine constrains the value to
    /// no published set.
    pub allowed_values: Option<String>,
    /// `Parameter$IsModifiable`.
    pub is_modifiable: bool,
    /// `Parameter$MinimumEngineVersion`.
    pub minimum_engine_version: String,
    /// `Parameter$ChangeType`: [`CHANGE_TYPE_IMMEDIATE`] or
    /// [`CHANGE_TYPE_REQUIRES_REBOOT`].
    pub change_type: String,
    /// `(CacheNodeType, Value)` for every node type this service publishes —
    /// non-empty exactly for a parameter whose value is per cache node type.
    pub node_type_values: Vec<(String, String)>,
}

impl ParameterRecord {
    /// Whether this parameter's value depends on the cache node type.
    pub fn is_node_type_specific(&self) -> bool {
        !self.node_type_values.is_empty()
    }

    /// The key this parameter is held and PAGED under.
    ///
    /// `EngineDefaults` carries two parameter lists and ONE `Marker`, so both are
    /// paged as one sequence: the plain parameters first, then the
    /// node-type-specific ones, each ordered by name. Prefixing the name with the
    /// list's position is what puts the whole catalog in the single total order a
    /// `Marker` resumes from.
    pub fn page_key(&self) -> String {
        parameter_page_key(&self.name, self.is_node_type_specific())
    }

    /// This record as the modelled `Parameter`.
    pub fn to_modeled(&self) -> Parameter {
        Parameter {
            parameter_name: Some(self.name.clone()),
            parameter_value: self.value.clone(),
            description: Some(self.description.clone()),
            source: Some(PARAMETER_SOURCE_SYSTEM.to_string()),
            data_type: Some(self.data_type.clone()),
            allowed_values: self.allowed_values.clone(),
            is_modifiable: Some(self.is_modifiable),
            minimum_engine_version: Some(self.minimum_engine_version.clone()),
            change_type: Some(self.change_type.clone()),
        }
    }

    /// The `Parameter$Source` a group reporting this parameter answers under:
    /// the customer's when the group holds a value for it, the service's
    /// otherwise; see [`CacheParameterGroupRecord::parameter_values`].
    ///
    /// A parameter a customer cannot set is never the customer's, whatever value
    /// the group holds for it: a parameter whose value is per cache node type
    /// takes the node's own capacity, and one the engine fixes
    /// (`IsModifiable: false`) can only carry a value the SERVICE put there —
    /// the cluster-mode flag of a cluster-mode-on default group, say.
    pub fn source_in_group(&self, set_value: Option<&str>) -> &'static str {
        if set_value.is_some() && !self.is_node_type_specific() && self.is_modifiable {
            PARAMETER_SOURCE_USER
        } else {
            PARAMETER_SOURCE_SYSTEM
        }
    }

    /// This record as the modelled `Parameter` of a group that holds
    /// `set_value` for it: the value the group was given, reported as the
    /// customer's, or the engine's own default reported as the service's.
    pub fn to_modeled_in_group(&self, set_value: Option<&str>) -> Parameter {
        let mut parameter = self.to_modeled();
        parameter.source = Some(self.source_in_group(set_value).to_string());
        if let Some(value) = set_value {
            parameter.parameter_value = Some(value.to_string());
        }
        parameter
    }

    /// This record as the modelled `CacheNodeTypeSpecificParameter`, for a
    /// parameter whose value is per cache node type.
    pub fn to_node_type_specific(&self) -> CacheNodeTypeSpecificParameter {
        CacheNodeTypeSpecificParameter {
            parameter_name: Some(self.name.clone()),
            description: Some(self.description.clone()),
            source: Some(PARAMETER_SOURCE_SYSTEM.to_string()),
            data_type: Some(self.data_type.clone()),
            allowed_values: self.allowed_values.clone(),
            is_modifiable: Some(self.is_modifiable),
            minimum_engine_version: Some(self.minimum_engine_version.clone()),
            cache_node_type_specific_values: Some(
                self.node_type_values
                    .iter()
                    .map(|(cache_node_type, value)| CacheNodeTypeSpecificValue {
                        cache_node_type: Some(cache_node_type.clone()),
                        value: Some(value.clone()),
                    })
                    .collect(),
            ),
            change_type: Some(self.change_type.clone()),
        }
    }
}

/// The key the parameter `name` is held and paged under, given which of
/// `EngineDefaults`' two lists reports it; see [`ParameterRecord::page_key`].
///
/// A free function because a lookup BY NAME has to build the same key without
/// holding the record it is looking for
/// ([`ComAmazonawsElasticacheAmazonelasticachev9State::family_parameter`]), and
/// two spellings of one key would let a write land where no read finds it.
fn parameter_page_key(name: &str, node_type_specific: bool) -> String {
    let list = if node_type_specific { 2 } else { 1 };
    format!("{list}:{name}")
}

/// `Parameter$DataType`'s spellings in the catalogs this service publishes: the
/// engine's own types, which are what decide whether a value a request asks to
/// set is a number at all and how it is compared against a published range.
pub const PARAMETER_DATA_TYPE_INTEGER: &str = "integer";
/// See [`PARAMETER_DATA_TYPE_INTEGER`].
pub const PARAMETER_DATA_TYPE_FLOAT: &str = "float";
/// See [`PARAMETER_DATA_TYPE_INTEGER`].
pub const PARAMETER_DATA_TYPE_BOOLEAN: &str = "boolean";
/// See [`PARAMETER_DATA_TYPE_INTEGER`].
pub const PARAMETER_DATA_TYPE_STRING: &str = "string";

/// `value` as the number a parameter of `data_type` takes, or `None` when it is
/// not one.
///
/// An `integer` (and a `boolean`, which the engines write as `0` / `1`) takes
/// only a whole number, so `1.5` is not one of its values; a `float` takes any
/// finite decimal number. A `string` parameter has no numeric reading at all.
/// The number is used only to COMPARE the value against a published range — the
/// text a request sent is what gets stored, so a value's own precision is never
/// re-rendered from this.
fn parameter_number(data_type: &str, value: &str) -> Option<f64> {
    let finite = value.parse::<f64>().ok().filter(|number| number.is_finite());
    match data_type {
        // The engines write a boolean as the flag `0` or `1`, and nothing else
        // is a value of that type, whether or not the row also publishes the
        // pair as its allowed values.
        PARAMETER_DATA_TYPE_BOOLEAN => finite.filter(|number| *number == 0.0 || *number == 1.0),
        PARAMETER_DATA_TYPE_INTEGER if value.parse::<i64>().is_err() => None,
        PARAMETER_DATA_TYPE_INTEGER | PARAMETER_DATA_TYPE_FLOAT => finite,
        _ => None,
    }
}

/// The bounds of `term` when it is a numeric RANGE of a published
/// `Parameter$AllowedValues` list — `<lo>-<hi>`, or `<lo>-` where the engine
/// bounds only the bottom — and `None` when it is a literal value.
///
/// A term is a range exactly when what precedes the `-` is a number, which is
/// what tells `1-75` apart from a literal that happens to carry a hyphen
/// (`volatile-lru`); see [`ParameterRow`]'s allowed-values column. A bound is
/// therefore never negative — every published bound is a count, a size or a
/// percentage — and a term written with one would read as a literal.
fn allowed_value_range(term: &str) -> Option<(f64, Option<f64>)> {
    let (low, high) = term.split_once('-')?;
    let low: f64 = low.parse().ok()?;
    if high.is_empty() {
        return Some((low, None));
    }
    Some((low, Some(high.parse().ok()?)))
}

/// Whether the engine takes `value` for the parameter `parameter` describes, or
/// the reason it does not.
///
/// The published catalog row IS the rule: a parameter the engine or the node
/// type fixes (`Parameter$IsModifiable: false`, and every node-type-specific
/// one) takes no customer value at all; a numeric parameter takes only a value
/// of its `Parameter$DataType`; and a parameter that publishes
/// `Parameter$AllowedValues` takes only a value that list covers — one of its
/// literal values, or a number inside one of its ranges. A parameter that
/// publishes no allowed values is constrained to no published set, so only its
/// data type is checked.
///
/// The refusal says which of those the value fell outside: the data type it is
/// not a value of, or the published set it is not in.
///
/// The one place a value a request asks to SET is held against the catalog, so
/// every operation that writes a parameter value applies the same rule.
pub fn check_parameter_value(parameter: &ParameterRecord, value: &str) -> Result<(), String> {
    check_parameter_modifiable(parameter)?;
    let number = parameter_number(&parameter.data_type, value);
    // A value that is not one of the parameter's own DATA TYPE is refused for
    // that, naming the type it is not a value of and the value that is not one:
    // it is outside every published set the engine could name, so the set is
    // not what the client got it wrong about.
    if parameter.data_type != PARAMETER_DATA_TYPE_STRING && number.is_none() {
        return Err(format!("invalid {} value: {value}", parameter.data_type));
    }
    let Some(allowed) = parameter.allowed_values.as_deref() else {
        // Constrained to no published set: the value is held only to the
        // parameter's own data type, which it already satisfies.
        return Ok(());
    };
    for term in allowed.split(',') {
        let covered = match allowed_value_range(term) {
            Some((low, high)) => number
                .is_some_and(|number| number >= low && high.is_none_or(|high| number <= high)),
            None => term == value,
        };
        if covered {
            return Ok(());
        }
    }
    // The refusal names the published set the value is outside of, which is the
    // parameter's own `Parameter$AllowedValues` as a read reports it: the client
    // that asked for the value learns which values it may ask for instead.
    Err(format!("{INVALID_PARAMETER_VALUE}, allowed values are:{allowed}"))
}

/// Whether a customer may change what the parameter `parameter` describes is
/// worth at all, or the reason the engine does not let them.
///
/// A parameter the engine fixes (`Parameter$IsModifiable: false`) and one whose
/// value is per cache node type both take whatever value the service or the
/// node's own capacity gives them, so neither a request SETTING a value nor one
/// RESETTING it to the engine default asks for a change that can be made. The
/// one place that rule is stated, so [`check_parameter_value`] and every
/// operation that only names a parameter word the same refusal.
pub fn check_parameter_modifiable(parameter: &ParameterRecord) -> Result<(), String> {
    if !parameter.is_modifiable || parameter.is_node_type_specific() {
        return Err(format!(
            "The parameter {} cannot be modified.",
            parameter.name
        ));
    }
    Ok(())
}

/// How ElastiCache opens the refusal of a value a parameter does not take; see
/// [`check_parameter_value`], which is where the published set the value is
/// outside of is appended to it.
const INVALID_PARAMETER_VALUE: &str = "invalid parameter value";

/// How ElastiCache refuses a request that names a parameter the group's family
/// publishes none of: the refusal names the parameter that was asked for, which
/// is the one thing the client got wrong.
///
/// The one place that refusal is worded, so a request SETTING such a parameter
/// and one RESETTING it answer a name the published catalog does not carry
/// identically.
pub fn unknown_parameter_message(name: &str) -> String {
    format!("Could not find parameter with name: {name}")
}

/// How many parameters one request may change: "A maximum of 20 parameters may
/// be modified per request"
/// (`ModifyCacheParameterGroup$ParameterNameValues`), which
/// `ResetCacheParameterGroup` names the same bound for.
pub const MAX_PARAMETERS_PER_REQUEST: usize = 20;

/// Every engine default parameter of `family`, keyed by
/// [`ParameterRecord::page_key`], or `None` when no published family carries
/// that name.
///
/// A family reports the rows of its engine's catalog whose minimum engine
/// version its line covers, so two families of one engine differ by exactly the
/// parameters the later versions introduced.
pub fn family_default_parameters(family: &str) -> Option<BTreeMap<String, ParameterRecord>> {
    let (engine, line) = family_line(family)?;
    let rows = if engine == PARAMETER_ENGINE_MEMCACHED {
        MEMCACHED_PARAMETERS
    } else {
        REDIS_PARAMETERS
    };
    let mut parameters = BTreeMap::new();
    for row in rows {
        let ParameterRow(
            name,
            value,
            description,
            data_type,
            allowed_values,
            is_modifiable,
            minimum_engine_version,
            change_type,
            node_type_specific,
        ) = *row;
        if version_rank(minimum_engine_version) > line {
            continue;
        }
        // The node types a node-type-specific parameter carries a value for are
        // [`OFFERING_NODE_TYPES`]' — the one list of cache node types this crate
        // publishes, so a parameter and an offering cannot disagree about which
        // node types exist.
        let node_type_values: Vec<(String, String)> = if node_type_specific {
            let values: Vec<(String, String)> = OFFERING_NODE_TYPES
                .iter()
                .filter_map(|(cache_node_type, _)| {
                    let (memory_mib, vcpus) = node_type_capacity(cache_node_type)?;
                    let value = node_type_specific_value(name, memory_mib, vcpus)?;
                    Some(((*cache_node_type).to_string(), value))
                })
                .collect();
            // A row flagged node-type-specific that no node type has a value
            // for would be reported as a plain parameter with NO value at all,
            // so it is refused here instead: it means a catalog row and
            // [`node_type_specific_value`] (or [`NODE_SIZES`] and a node type
            // added to [`OFFERING_NODE_TYPES`]) disagree, which is a bug in this
            // module rather than anything a request can ask for — the same
            // reading as [`offering_rate_share`]'s `expect`s.
            assert!(
                values.len() == OFFERING_NODE_TYPES.len(),
                "the node-type-specific parameter {name} must carry a value for every node \
                 type of OFFERING_NODE_TYPES, got {}",
                values.len()
            );
            values
        } else {
            Vec::new()
        };
        let record = ParameterRecord {
            name: name.to_string(),
            value: (!node_type_specific).then(|| value.to_string()),
            description: description.to_string(),
            data_type: data_type.to_string(),
            allowed_values: (!allowed_values.is_empty()).then(|| allowed_values.to_string()),
            is_modifiable,
            minimum_engine_version: minimum_engine_version.to_string(),
            change_type: change_type.to_string(),
            node_type_values,
        };
        parameters.insert(record.page_key(), record);
    }
    Some(parameters)
}

/// The engine default parameters of every published family, keyed by family.
///
/// Seeded into
/// [`ComAmazonawsElasticacheAmazonelasticachev9State::engine_default_parameters`]
/// when the store is created: the engine's defaults are the service's own
/// catalog, which exists before anything has been created and which no operation
/// creates or removes an entry from.
pub fn published_engine_default_parameters() -> BTreeMap<String, BTreeMap<String, ParameterRecord>>
{
    PARAMETER_GROUP_FAMILIES
        .iter()
        .filter_map(|family| {
            family_default_parameters(family).map(|parameters| ((*family).to_string(), parameters))
        })
        .collect()
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

/// How many hex characters an identifier of the account's EC2 network carries:
/// a subnet its long form (`subnet-0bb1c79de3example`), a VPC EC2's short one
/// (`vpc-0a1b2c3d`).
const SUBNET_ID_HEX_LEN: usize = 17;
/// See [`SUBNET_ID_HEX_LEN`].
const VPC_ID_HEX_LEN: usize = 8;

/// One subnet of the account's EC2 network in one Region: the identifier a
/// request names, the VPC it belongs to and the Availability Zone it sits in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountSubnet {
    /// The EC2 subnet identifier, as a request names it.
    pub subnet_id: String,
    /// The VPC this subnet belongs to.
    pub vpc_id: String,
    /// The Availability Zone this subnet sits in.
    pub availability_zone: String,
}

/// The id of the account's default VPC in `region`.
///
/// AWS gives an account a default VPC in every Region and this emulator models
/// no other, so the id is DERIVED from the Region name — stable, so every read
/// reports the same VPC, and holding no state, so nothing has to be seeded or
/// reset. This is the contract `services/common/src/network.rs` states for
/// every service that places a resource in a subnet, implemented here constant
/// for constant, so a client that creates an ElastiCache subnet group and a DAX
/// one "in the same subnet" is talking to one account.
pub fn default_vpc_id(region: &str) -> String {
    use ::sha2::Digest;
    let digest = ::hex::encode(::sha2::Sha256::digest(region.as_bytes()));
    format!("vpc-{}", &digest[..VPC_ID_HEX_LEN])
}

/// How many Availability Zones a Region's default VPC has a default subnet in.
///
/// The account's network is the SAME fact for every service that places a
/// resource in a subnet, so this is the count
/// `services/common/src/network.rs` states — "a Region's default subnets sit one
/// per Availability Zone, in the zones named `<region>a`, `<region>b`,
/// `<region>c`" — and NOT the number of them ElastiCache selects for a
/// serverless cache that names none ([`default_subnet_count`], which selects a
/// SUBSET of these).
const DEFAULT_SUBNETS_PER_REGION: usize = 3;

/// The subnets that exist in `region`: the default VPC's default subnet in each
/// Availability Zone of that Region.
///
/// AWS creates the default VPC with "a size /16 IPv4 CIDR block and a default
/// subnet in each Availability Zone" (EC2's `CreateDefaultVpc`), so these
/// subnets are facts of the account's Region rather than resources an
/// ElastiCache request creates, and each sits in the zone [`availability_zone`]
/// names. Each identifier is derived from its zone name, so a Region always
/// reports the same subnets and [`resolve_subnet`] can tell an identifier that
/// names one from an identifier that names nothing at all.
///
/// The subnets ElastiCache SELECTS for a serverless cache that names none are
/// the first [`default_subnet_count`] of these, so what it selects is always
/// something a request could have named itself.
pub fn region_subnets(region: &str) -> Vec<AccountSubnet> {
    use ::sha2::Digest;
    let vpc_id = default_vpc_id(region);
    (0..DEFAULT_SUBNETS_PER_REGION)
        .map(|index| {
            let availability_zone = availability_zone(region, index);
            let digest = ::hex::encode(::sha2::Sha256::digest(availability_zone.as_bytes()));
            AccountSubnet {
                subnet_id: format!("subnet-{}", &digest[..SUBNET_ID_HEX_LEN]),
                vpc_id: vpc_id.clone(),
                availability_zone,
            }
        })
        .collect()
}

/// The subnets ElastiCache selects for a resource in `region` whose request
/// named none: the first [`default_subnet_count`] of the Region's own
/// ([`region_subnets`]).
pub fn selected_default_subnets(region: &str) -> Vec<AccountSubnet> {
    let mut subnets = region_subnets(region);
    subnets.truncate(default_subnet_count(region));
    subnets
}

/// The subnet `subnet_id` names in `region`, or `None` when no subnet of that
/// Region carries that identifier — which is what lets a request naming it be
/// refused instead of stored.
///
/// EC2's own subnet operations still answer 501 in this emulator; when they are
/// implemented, this is the one place that has to start consulting the stored
/// subnets beside the derived ones.
pub fn resolve_subnet(region: &str, subnet_id: &str) -> Option<AccountSubnet> {
    region_subnets(region)
        .into_iter()
        .find(|subnet| subnet.subnet_id == subnet_id)
}

/// The subnet ids among `subnet_ids` that name no subnet of `region`, in the
/// order the request listed them.
pub fn unresolved_subnets<'a, I>(region: &str, subnet_ids: I) -> Vec<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    let known = region_subnets(region);
    subnet_ids
        .into_iter()
        .filter(|id| !known.iter().any(|subnet| subnet.subnet_id == *id))
        .collect()
}

/// How ElastiCache words a request naming subnets the account does not have,
/// built from the offending ids: `Some input subnets in :[<ids>] are invalid.`
pub fn invalid_subnets_message(subnet_ids: &[&str]) -> String {
    format!("Some input subnets in :[{}] are invalid.", subnet_ids.join(", "))
}

/// The length bound a LOOKUP applies to an identifier before resolving it:
/// ElastiCache's own identifier bound (`CacheClusterId`: "A name must contain
/// from 1 to 50 alphanumeric characters or hyphens"), which is also the bound
/// the service states when it refuses an over-long `ReplicationGroupId` on a
/// describe ("Replication Group ID cannot be longer than 50 characters").
///
/// A CREATE applies the stricter bound its own member documents — see
/// [`REPLICATION_GROUP_ID_MAX_LEN`], 40 characters, because creating a
/// replication group also mints the member cluster ids from the group id — so
/// an id between the two bounds is refused by a create and merely not found by
/// a lookup.
pub const LOOKUP_IDENTIFIER_MAX_LEN: usize = CLUSTER_ID_MAX_LEN;

/// Why `value` cannot name a resource of this service, or `None` when it can:
/// the refusal a lookup answers BEFORE it resolves the identifier, so a value
/// that no resource could ever carry is reported as the bad parameter it is
/// rather than as a resource that does not exist.
///
/// `prose` is how the service names the member in the length refusal
/// ("Replication Group ID"), `parameter` how it names it in the identifier-rule
/// refusal ("ReplicationGroupIdentifier"); both texts are the service's own.
pub fn identifier_refusal(
    prose: &str,
    parameter: &str,
    value: &str,
    max_len: usize,
) -> Option<String> {
    if value.len() > max_len {
        return Some(format!("{prose} cannot be longer than {max_len} characters."));
    }
    if is_valid_identifier(value, max_len) {
        return None;
    }
    Some(format!(
        "The parameter {parameter} is not a valid identifier. Identifiers must begin with a \
         letter; must contain only ASCII letters, digits, and hyphens; and must not end with a \
         hyphen or contain two consecutive hyphens."
    ))
}

/// How `CreateSnapshot` words the cache cluster it is asked to snapshot when
/// this account holds no such cluster: `Cache cluster not found: <id>`, the
/// resource named as PROSE rather than as the shape.
///
/// The snapshot creator words this fault its own way, as the two writers of
/// parameter values word theirs (see
/// [`parameter_writer_group_not_found_message`]); the id is reported as the
/// request spelled it, because the message is built from the request.
pub fn snapshot_source_cluster_not_found_message(cache_cluster_id: &str) -> String {
    format!("Cache cluster not found: {cache_cluster_id}")
}

/// How ElastiCache words a cache cluster this account does not hold:
/// `CacheCluster not found: <id>`.
///
/// One helper, so every surface that reports a missing cache cluster — the
/// modelled `CacheClusterNotFoundFault` of a describe, modify, delete, reboot or
/// snapshot call, and the `ErrorMessage` a batch update action reports for a
/// resource it could not resolve — words it the one way the service does.
pub fn cache_cluster_not_found_message(cache_cluster_id: &str) -> String {
    format!("CacheCluster not found: {cache_cluster_id}")
}

/// How ElastiCache words a replication group this account does not hold:
/// `ReplicationGroup <id> not found.`
///
/// One helper, so every surface that answers `ReplicationGroupNotFoundFault` for
/// an id it could not resolve — the describe, the modifies, the failover test,
/// the creates that join a group, and the `ErrorMessage` a batch update action
/// reports — words it the one way the service does. The resource is named the way
/// the SHAPE spells it (`ReplicationGroup`), and the id is reported as the
/// request spelled it, because the message is built from the request.
pub fn replication_group_not_found_message(replication_group_id: &str) -> String {
    format!("ReplicationGroup {replication_group_id} not found.")
}

/// How ElastiCache words a global replication group (a Global datastore) this
/// account does not hold.
///
/// The template names NO identifier: the service states the class of resource
/// that could not be resolved and nothing else, so interpolating the requested id
/// would change the string for every caller.
pub const GLOBAL_REPLICATION_GROUP_NOT_FOUND_MESSAGE: &str = "Global Replication Group not found";

/// How ElastiCache words a `CreateReplicationGroup` whose `ReplicationGroupId` is
/// already taken. Like [`SNAPSHOT_ALREADY_EXISTS_MESSAGE`], the template names the
/// colliding name only as "specified name" rather than interpolating it.
pub const REPLICATION_GROUP_ALREADY_EXISTS_MESSAGE: &str =
    "Replication group with specified name already exists.";

/// How ElastiCache words a snapshot name that is already taken, whether the
/// snapshot would be taken from a cluster or copied from another snapshot; see
/// [`REPLICATION_GROUP_ALREADY_EXISTS_MESSAGE`].
pub const SNAPSHOT_ALREADY_EXISTS_MESSAGE: &str = "Snapshot with specified name already exists.";

/// How ElastiCache words a service update its published catalogue does not hold:
/// `ServiceUpdate <name> not found.`
///
/// One helper for the describe and the two batch update-action operations, so the
/// three cannot disagree. The resource is named the way the SHAPE spells it
/// (`ServiceUpdate`), not as English prose.
pub fn service_update_not_found_message(service_update_name: &str) -> String {
    format!("ServiceUpdate {service_update_name} not found.")
}

/// How ElastiCache words a `CacheParameterGroupName` that is already taken:
/// `Parameter group <name> already exists` — no trailing stop.
pub fn cache_parameter_group_already_exists_message(name: &str) -> String {
    format!("Parameter group {name} already exists")
}

/// How ElastiCache words a cache parameter group this account does not hold:
/// `CacheParameterGroup <name> not found.`
///
/// One helper, so every surface that resolves a `CacheParameterGroupName` — the
/// describes, the delete, and the creates and modifies that take a group for a
/// cluster or a replication group — words its `CacheParameterGroupNotFoundFault`
/// the one way the service does. The name is reported as the request spelled it,
/// because the message is built from the request.
///
/// The two operations that WRITE parameter values word the same fault the other
/// way round; see [`parameter_writer_group_not_found_message`].
pub fn cache_parameter_group_not_found_message(name: &str) -> String {
    format!("CacheParameterGroup {name} not found.")
}

/// How `ModifyCacheParameterGroup` and `ResetCacheParameterGroup` word a cache
/// parameter group this account does not hold: `CacheParameterGroup not found:
/// <name>`, the form the service's parameter writers answer with (and the form
/// [`cache_cluster_not_found_message`] carries).
///
/// One helper, so the two writers of parameter values cannot disagree about it.
pub fn parameter_writer_group_not_found_message(name: &str) -> String {
    format!("CacheParameterGroup not found: {name}")
}

/// How ElastiCache refuses a change to a family's OWN default parameter group,
/// whether the request asks to set a value or to put one back: the group's
/// values are the engine's, so there is no change to make. Stated once, so the
/// two writers of parameter values cannot disagree about it.
pub const DEFAULT_PARAMETER_GROUP_IMMUTABLE: &str = "Cannot modify a default parameter group.";

/// How ElastiCache words a `CacheParameterGroupFamily` that names none of the
/// families it publishes: `CacheParameterGroupFamily <family> is not a valid
/// parameter group family.`
///
/// The service answers the RULE the value broke — the family vocabulary is the
/// service's own, so a name outside it names no catalog — rather than listing
/// the families back. The requested family is the one variable part, taken from
/// the request. One helper, so every operation resolving a family (the engine
/// defaults describe and the group create) words it the one way the service
/// does; the published vocabulary itself is [`PARAMETER_GROUP_FAMILIES`].
pub fn invalid_parameter_group_family_message(family: &str) -> String {
    format!("CacheParameterGroupFamily {family} is not a valid parameter group family.")
}

/// How ElastiCache words a snapshot this account does not hold: the name the
/// request asked for, which is all the service knows about a snapshot that does
/// not exist.
///
/// One helper, so every operation that acts on a NAMED snapshot
/// (`DeleteSnapshot`, `CopySnapshot`, and the tag operations when they land)
/// words its `SnapshotNotFoundFault` the one way the service does. The name is
/// reported as the caller sent it rather than lowercased, because the message is
/// built from the request.
pub fn snapshot_not_found_message(snapshot_name: &str) -> String {
    format!("Snapshot with name {snapshot_name} not found")
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

/// The ARN of a cache parameter group:
/// `arn:aws:elasticache:<region>:<account>:parametergroup:<name>`.
pub fn parameter_group_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("parametergroup:{name}"))
}

/// The ARN of a snapshot: `arn:aws:elasticache:<region>:<account>:snapshot:<name>`.
pub fn snapshot_arn(region: &str, snapshot_name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("snapshot:{snapshot_name}"))
}

/// The kind of resource an ElastiCache ARN names, which is what decides WHICH
/// not-found fault a tagging call answers with for an ARN this store holds no
/// resource for: each of `AddTagsToResource`, `ListTagsForResource` and
/// `RemoveTagsFromResource` models one per kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggedResourceKind {
    CacheCluster,
    CacheParameterGroup,
    CacheSecurityGroup,
    CacheSubnetGroup,
    GlobalReplicationGroup,
    ReplicationGroup,
    ReservedCacheNode,
    ServerlessCache,
    ServerlessCacheSnapshot,
    Snapshot,
    User,
    UserGroup,
}

impl TaggedResourceKind {
    /// How this kind is named in the message of the fault it answers with.
    pub fn label(self) -> &'static str {
        match self {
            Self::CacheCluster => "Cache cluster",
            Self::CacheParameterGroup => "Cache parameter group",
            Self::CacheSecurityGroup => "Cache security group",
            Self::CacheSubnetGroup => "Cache subnet group",
            Self::GlobalReplicationGroup => "Global datastore",
            Self::ReplicationGroup => "Replication group",
            Self::ReservedCacheNode => "Reserved cache node",
            Self::ServerlessCache => "Serverless cache",
            Self::ServerlessCacheSnapshot => "Serverless cache snapshot",
            Self::Snapshot => "Snapshot",
            Self::User => "User",
            Self::UserGroup => "User group",
        }
    }
}

/// The resource-type segment an ElastiCache ARN carries for each kind.
///
/// Every segment this service's ARN helpers mint ([`cluster_arn`],
/// [`subnet_group_arn`], [`parameter_group_arn`], [`snapshot_arn`],
/// [`replication_group_arn`], [`reservation_arn`], [`serverless_cache_arn`],
/// [`serverless_cache_snapshot_arn`], [`user_arn`], [`user_group_arn`],
/// [`global_replication_group_arn`]), plus the one kind this emulator mints no
/// resource of yet — the EC2-Classic cache security group. A segment in neither
/// list names no ElastiCache resource at all, which is what [`parse_resource_arn`]
/// answers `None` for.
const RESOURCE_TYPE_SEGMENTS: &[(&str, TaggedResourceKind)] = &[
    ("cluster", TaggedResourceKind::CacheCluster),
    ("parametergroup", TaggedResourceKind::CacheParameterGroup),
    ("securitygroup", TaggedResourceKind::CacheSecurityGroup),
    ("subnetgroup", TaggedResourceKind::CacheSubnetGroup),
    (
        "globalreplicationgroup",
        TaggedResourceKind::GlobalReplicationGroup,
    ),
    ("replicationgroup", TaggedResourceKind::ReplicationGroup),
    ("reserved-instance", TaggedResourceKind::ReservedCacheNode),
    ("serverlesscache", TaggedResourceKind::ServerlessCache),
    (
        "serverlesscachesnapshot",
        TaggedResourceKind::ServerlessCacheSnapshot,
    ),
    ("snapshot", TaggedResourceKind::Snapshot),
    ("user", TaggedResourceKind::User),
    ("usergroup", TaggedResourceKind::UserGroup),
];

/// What an ElastiCache resource ARN carries: the Region it names the resource
/// in, the kind of resource, and the resource's own name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceArnParts<'a> {
    pub region: &'a str,
    pub kind: TaggedResourceKind,
    pub name: &'a str,
}

impl ResourceArnParts<'_> {
    /// The rejection an ARN of this kind that names no stored resource is
    /// answered with: the kind decides which modelled not-found fault the
    /// calling operation lifts it into.
    pub fn not_found(&self) -> TagTargetRejection {
        TagTargetRejection::NotFound(self.kind, format!("{} is not present", self.name))
    }
}

/// The parts of an ElastiCache resource ARN, or `None` when `resource_arn` is
/// not one.
///
/// The shape is the one every ARN this service mints has (see [`cluster_arn`]):
/// `arn:<partition>:elasticache:<region>:<account>:<type>:<name>`, with a
/// non-empty partition, Region and account, a resource type from
/// [`RESOURCE_TYPE_SEGMENTS`] and a non-empty resource name. The service field
/// must be this service's ([`ARN_NAMESPACE`]): another service's ARN names no
/// ElastiCache resource.
pub fn parse_resource_arn(resource_arn: &str) -> Option<ResourceArnParts<'_>> {
    let mut fields = resource_arn.splitn(6, ':');
    let prefix = fields.next()?;
    let partition = fields.next()?;
    let service = fields.next()?;
    let region = fields.next()?;
    let account = fields.next()?;
    let resource = fields.next()?;
    if prefix != "arn"
        || partition.is_empty()
        || service != ARN_NAMESPACE
        || region.is_empty()
        || account.is_empty()
    {
        return None;
    }
    let (resource_type, name) = resource.split_once(':')?;
    if name.is_empty() {
        return None;
    }
    let (_, kind) = RESOURCE_TYPE_SEGMENTS
        .iter()
        .find(|(segment, _)| *segment == resource_type)?;
    Some(ResourceArnParts {
        region,
        kind: *kind,
        name,
    })
}

/// Why a tagging call's `ResourceName` names no resource of this service (see
/// [`ComAmazonawsElasticacheAmazonelasticachev9State::resource_tags`]).
///
/// Carries the MESSAGE and, for a well-formed ARN, the kind — not a modelled
/// error: each of the three tagging operations declares its own error enum, so
/// each lifts this into its own `InvalidARNFault` / per-kind not-found variant.
#[derive(Debug, Clone)]
pub enum TagTargetRejection {
    /// The value is not an ElastiCache resource ARN at all: the 400
    /// `InvalidARNFault`, "the requested Amazon Resource Name (ARN) does not
    /// refer to an existing resource".
    InvalidArn(String),
    /// A well-formed ARN naming no resource in this store: that kind's own
    /// not-found fault.
    NotFound(TaggedResourceKind, String),
}

/// How many colon-separated components an ElastiCache resource ARN has:
/// `arn:<partition>:elasticache:<region>:<account>:<type>:<name>` — the form
/// every ARN this service mints carries (see [`parse_resource_arn`]).
pub const RESOURCE_ARN_COMPONENTS: usize = 7;

/// How ElastiCache words a `ResourceName` that names no resource of this
/// service: it reports the STRUCTURAL rule it applied and interpolates no
/// caller value at all.
///
/// The rule a value that is not ARN-shaped breaks is the component count, which
/// is the only part of the form that can be counted without interpreting any
/// field; a value that does carry the components but names no ElastiCache
/// resource is answered with the rule the modelled `InvalidARNFault` itself
/// states ("The requested Amazon Resource Name (ARN) does not refer to an
/// existing resource"). Neither template carries the request's value, so the
/// message is the same sentence for every caller.
pub fn invalid_resource_arn_message(resource_arn: &str) -> String {
    if resource_arn.split(':').count() != RESOURCE_ARN_COMPONENTS {
        return format!("Input ARN string does not have {RESOURCE_ARN_COMPONENTS} components.");
    }
    "The requested Amazon Resource Name (ARN) does not refer to an existing resource.".to_string()
}

/// `resource_arn` parsed, or the `InvalidArn` rejection for a value that is not
/// an ElastiCache resource ARN.
fn tag_target_parts(resource_arn: &str) -> Result<ResourceArnParts<'_>, TagTargetRejection> {
    parse_resource_arn(resource_arn).ok_or_else(|| {
        TagTargetRejection::InvalidArn(invalid_resource_arn_message(resource_arn))
    })
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

/// The page size a describe serves, or the reason the page-size member is
/// refused.
///
/// A page-size member carries no modelled `@range`, so the boundary validator
/// passes any integer and the bounds the asking operation's own member
/// documents are enforced here — `member` names it in the refusal, and
/// `default`, `min` and `max` are that operation's. It is refused rather than
/// clamped: a caller that asked for a page size the service does not serve gets
/// told so instead of a silently different page.
fn page_size_bounded(
    requested: Option<i32>,
    member: &str,
    default: i32,
    min: i32,
    max: i32,
) -> Result<usize, String> {
    let requested = requested.unwrap_or(default);
    if !(min..=max).contains(&requested) {
        // The wording ElastiCache itself answers a page size outside the bounds
        // its member documents with: the member's own constraint, stated in the
        // service's words, with the operation's own maximum. `MaxRecords` keeps
        // the recorded wording exactly; another member names itself.
        return Err(if member == "MaxRecords" {
            format!("Invalid value for max records. Must be between {min} and {max}")
        } else {
            format!(
                "Invalid value {requested} for {member}: must be at least {min} and no more than \
                 {max}."
            )
        });
    }
    Ok(usize::try_from(requested).unwrap_or(min.max(1) as usize))
}

/// The page size a `MaxRecords` describe serves; see [`page_size_bounded`]. The
/// minimum is the [`MAX_RECORDS_MIN`] every `MaxRecords` member in this service
/// documents.
pub fn page_size_within(max_records: Option<i32>, default: i32, max: i32) -> Result<usize, String> {
    page_size_bounded(max_records, "MaxRecords", default, MAX_RECORDS_MIN, max)
}

/// The page size `DescribeCacheClusters` / `DescribeCacheSubnetGroups` /
/// `DescribeCacheParameterGroups` serve, whose `MaxRecords` documents "Default:
/// 100. Constraints: minimum 20; maximum 100".
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

/// The page size `DescribeServerlessCacheSnapshots` serves, whose `MaxResults`
/// documents "The default is 50. The Validation Constraints are a maximum of
/// 50." — a maximum with no stated minimum, so the smallest page it serves is
/// one record; see [`SERVERLESS_SNAPSHOT_MAX_RESULTS_DEFAULT`].
pub fn serverless_snapshot_page_size(max_results: Option<i32>) -> Result<usize, String> {
    page_size_bounded(
        max_results,
        "MaxResults",
        SERVERLESS_SNAPSHOT_MAX_RESULTS_DEFAULT,
        1,
        SERVERLESS_SNAPSHOT_MAX_RESULTS_MAX,
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

/// How ElastiCache refuses an `Engine` a serverless cache cannot run: the
/// CAPABILITY rule, stating that the engine named does not support serverless
/// caches at all.
///
/// Wholly fixed text with no request value: the rule is about the engine's
/// capability, not about a vocabulary the caller could pick another entry from.
pub const SERVERLESS_ENGINE_UNSUPPORTED: &str = "Specified engine does not support serverless cache.";

/// `engine` lowercased when it names an engine a serverless cache can run, else
/// the capability refusal ([`SERVERLESS_ENGINE_UNSUPPORTED`]).
///
/// The engines a serverless cache runs are the ones
/// `ModifyServerlessCache$Engine` documents ("The options are valkey, memcached
/// or redis" \u2014 `smithy_query(query="doc", shape="ModifyServerlessCacheRequest",
/// member="Engine")`), which is [`ENGINES`]; anything else is an engine without
/// the capability rather than a value outside a list. One implementation, so the
/// create and the modify of a serverless cache cannot disagree about it.
pub fn normalize_serverless_engine(engine: &str) -> Result<String, String> {
    let lowered = engine.to_ascii_lowercase();
    if ENGINES.contains(&lowered.as_str()) {
        Ok(lowered)
    } else {
        Err(SERVERLESS_ENGINE_UNSUPPORTED.to_string())
    }
}

/// How ElastiCache words a cache engine name that names no engine at all, which
/// is what the engine-and-version DISCOVERY describe answers for an `Engine`
/// filter the published catalog holds nothing for: `Invalid engine name: <name>`.
///
/// Distinct from [`normalize_engine`], which is how the operations that CREATE a
/// resource running an engine word a value that is not one they serve.
pub fn invalid_engine_name_message(engine: &str) -> String {
    format!("Invalid engine name: {engine}")
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

/// A status change that has just been materialised by a settle: the instant it
/// fell due, the status the record carried until then, and the one it carries
/// from then on.
///
/// Handed back so the one place a transition becomes real is also the place the
/// event recording it is written, dated at the instant the clock owed the
/// change rather than at the call that happened to notice it.
#[derive(Debug, Clone)]
pub struct SettledTransition {
    pub due: SystemTime,
    pub from: String,
    pub to: String,
}

/// One event this service recorded: something that happened to a resource of
/// it, at the instant it happened.
///
/// The log is the one copy `DescribeEvents` reports, and it outlives the
/// resource an event is about: ElastiCache still reports the events of a
/// cluster that has since been deleted.
#[derive(Debug, Clone)]
pub struct EventRecord {
    /// `Event$SourceIdentifier`: "the identifier for the source of the event.
    /// For example, if the event occurred at the cluster level, the identifier
    /// would be the name of the cluster."
    pub source_identifier: String,
    /// One of [`SOURCE_TYPE_CACHE_CLUSTER`] / [`SOURCE_TYPE_CACHE_SUBNET_GROUP`].
    pub source_type: String,
    pub message: String,
    /// `Event$Date`: "the date and time when the event occurred."
    pub at: SystemTime,
}

impl EventRecord {
    /// This record as the modelled `Event`.
    pub fn to_modeled(&self) -> Event {
        Event {
            source_identifier: Some(self.source_identifier.clone()),
            source_type: Some(self.source_type.clone()),
            message: Some(self.message.clone()),
            date: Some(::aws_smithy_types::DateTime::from(self.at)),
        }
    }
}

/// The key an event is stored under: the instant it happened, then the sequence
/// it was recorded in.
///
/// Ordering the key by time is what puts the log in the chronological order
/// `DescribeEvents` reports and its `Marker` indexes into; the sequence breaks
/// the tie between two events recorded within the same millisecond, so no event
/// can displace another. Both fields are zero-padded so the `BTreeMap`'s
/// lexicographic order over the keys IS their numeric order.
pub fn event_key(at: SystemTime, seq: u64) -> String {
    let millis = at
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("{millis:020}-{seq:020}")
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
/// `RecurringCharges` entry. EVERY offering publishes that entry, so
/// `RecurringCharges` is always a `RecurringCharge` structure a client can read
/// the frequency and the amount off: an all-upfront offering's amount is 0,
/// because its whole term is already in the fixed price, which is the one
/// combination that pairs with a non-zero `FixedPrice`. `UsagePrice`, "the
/// hourly price charged for this offering", is 0 for all three: an upfront-type
/// reservation's hourly charge is the recurring charge, not a usage price.
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
    let recurring_charges = vec![RecurringCharge {
        recurring_charge_amount: Some(round_to(recurring_hourly, 4)),
        recurring_charge_frequency: Some(RECURRING_CHARGE_HOURLY.to_string()),
    }];
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
    /// The tags the purchase was given. The ONE copy of this reservation's tags:
    /// `ListTagsForResource` reports it and `AddTagsToResource` /
    /// `RemoveTagsFromResource` change it, through
    /// [`ComAmazonawsElasticacheAmazonelasticachev9State::resource_tags`].
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
    /// `CacheNode$ParameterGroupStatus`: [`PARAMETER_APPLY_STATUS`] while this
    /// node runs the current values of its cluster's parameter group,
    /// [`PARAMETER_APPLY_STATUS_PENDING_REBOOT`] once a parameter the engine
    /// applies only at a restart has been changed on that group. Held per NODE
    /// because a reboot of some of a cluster's nodes applies the group to those
    /// nodes alone, and the cluster's own status is read off its nodes (see
    /// [`CacheClusterRecord::parameter_apply_status`]).
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
            // handler rather than deferred, so neither is ever pending on a
            // CLUSTER; a member cluster's in-transit encryption is the group's,
            // and `ModifyReplicationGroup` reports what it defers under the
            // GROUP's own pending values.
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
    /// The tags `CreateCacheCluster` was given. The ONE copy of this cluster's
    /// tags: `ListTagsForResource` reports it and `AddTagsToResource` /
    /// `RemoveTagsFromResource` change it, through
    /// [`ComAmazonawsElasticacheAmazonelasticachev9State::resource_tags`].
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
        let target = target.clamp(MIN_CACHE_NODES_PER_CLUSTER, MAX_CACHE_NODES_PER_CLUSTER);
        if !ids_to_remove.is_empty() {
            self.nodes
                .retain(|node| !ids_to_remove.contains(&node.cache_node_id));
        }
        // The growth loop below allocates one record per node, so the node
        // count is bounded at the top of this function — where both the create
        // and the modify path go through it — rather than left to each caller's
        // own validation: no engine permits more than
        // [`MAX_CACHE_NODES_PER_CLUSTER`] nodes and no cluster has fewer than
        // [`MIN_CACHE_NODES_PER_CLUSTER`] (see [`validate_num_cache_nodes`]), so
        // a count outside that range cannot be a cluster this service serves and
        // is clamped instead of allocated.
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
    /// past its due instant, and hand back the change it made.
    ///
    /// This is the ONE write a reporting operation may make: the transition was
    /// owed at this instant whether or not anybody called. Nothing else about
    /// the cluster moves. `None` means the clock owed nothing.
    pub fn settle(&mut self, now: SystemTime) -> Option<SettledTransition> {
        let pending = self.pending_status.as_ref()?;
        if now < pending.due {
            return None;
        }
        let settled = SettledTransition {
            due: pending.due,
            from: self.status.clone(),
            to: pending.status.clone(),
        };
        self.status = pending.status.clone();
        self.pending_status = None;
        if self.status == STATUS_AVAILABLE {
            self.set_node_status(STATUS_AVAILABLE);
        }
        Some(settled)
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

    /// What `CacheParameterGroupStatus$ParameterApplyStatus` reports for this
    /// cluster's association with its parameter group: the group's values are
    /// applied to the cluster only once every node runs them, so a cluster with
    /// any node still owed a reboot is pending one; see
    /// [`CacheNodeRecord::parameter_group_status`].
    pub fn parameter_apply_status(&self) -> &'static str {
        if self.cache_node_ids_to_reboot().is_empty() {
            PARAMETER_APPLY_STATUS
        } else {
            PARAMETER_APPLY_STATUS_PENDING_REBOOT
        }
    }

    /// The nodes `CacheParameterGroupStatus$CacheNodeIdsToReboot` names: "a
    /// list of the cache node IDs which need to be rebooted for parameter
    /// changes to be applied", in the cluster's own node order.
    pub fn cache_node_ids_to_reboot(&self) -> Vec<String> {
        self.nodes
            .iter()
            .filter(|node| node.parameter_group_status == PARAMETER_APPLY_STATUS_PENDING_REBOOT)
            .map(|node| node.cache_node_id.clone())
            .collect()
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
            // exist here: the whole surface is closed for this account, see
            // [`CACHE_SECURITY_GROUPS_UNSUPPORTED`].
            cache_security_groups: None,
            cache_parameter_group: Some(CacheParameterGroupStatus {
                cache_parameter_group_name: Some(self.cache_parameter_group_name.clone()),
                parameter_apply_status: Some(self.parameter_apply_status().to_string()),
                // "A list of the cache node IDs which need to be rebooted for
                // parameter changes to be applied": nothing to name while the
                // nodes already run the group's current values.
                cache_node_ids_to_reboot: {
                    let ids = self.cache_node_ids_to_reboot();
                    (!ids.is_empty()).then_some(ids)
                },
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

/// The smallest `NewReplicaCount` `IncreaseReplicaCount` accepts: an increase
/// ends with at least one replica, so a request to end with none asks this
/// operation for something it cannot do.
pub const INCREASE_REPLICA_COUNT_MIN: i32 = 1;

/// The smallest `NewReplicaCount` `DecreaseReplicaCount` accepts: "The minimum
/// number of replicas in a shard or replication group is … if Multi-AZ is not
/// enabled: 0" (`DecreaseReplicaCount$NewReplicaCount`), so a decrease may take a
/// shard down to none.
pub const DECREASE_REPLICA_COUNT_MIN: i32 = 0;

/// How ElastiCache states the replica-count range a request fell outside of: the
/// bounds the CALLING operation holds the count to, and nothing of the value that
/// missed them.
pub fn replica_count_range_message(minimum: i32) -> String {
    format!(
        "New replica count must be between {minimum} and {MAX_REPLICAS_PER_NODE_GROUP} replicas."
    )
}

/// How ElastiCache refuses `NumCacheClusters` on a request that also asks for
/// more than one shard: the member "is not used if there is more than one node
/// group (shard)", which is the rule the message states.
pub const NUM_CACHE_CLUSTERS_ONE_SHARD_MESSAGE: &str =
    "NumCacheClusters can only be specified for one node group.";

/// The largest `NumNodeGroups`, from
/// `NodeGroupsPerReplicationGroupQuotaExceededFault`: "The request cannot be
/// processed because it would exceed the maximum allowed number of node groups
/// (shards) in a single replication group. The default maximum is 90".
pub const MAX_NODE_GROUPS: i32 = 90;

/// How many hash slots a Valkey / Redis OSS keyspace holds, from
/// `NodeGroupConfiguration$Slots`: "Keyspaces range from 0 to 16,383."
pub const KEYSPACE_SLOTS: i32 = 16_384;

/// How many shards an account may test automatic failover on in one rolling
/// window, as `TestFailover` documents it: "A customer can use this operation to
/// test automatic failover on up to 15 shards (called node groups in the
/// ElastiCache API and Amazon CLI) in any rolling 24-hour period." Past it the
/// call is refused with the modelled `APICallRateForCustomerExceededFault`.
pub const MAX_TEST_FAILOVERS_PER_WINDOW: usize = 15;

/// The window [`MAX_TEST_FAILOVERS_PER_WINDOW`] is counted over, from the same
/// sentence: "any rolling 24-hour period".
pub const TEST_FAILOVER_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

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

/// `ClusterMode`'s third value (`smithy_query(query="shape",
/// shape="ClusterMode")`: `enabled | disabled | compatible`), the step a group
/// has to pass through to reach `enabled`: "To modify cluster mode from
/// Disabled to Enabled, you must first set the cluster mode to Compatible."
pub const CLUSTER_MODE_COMPATIBLE: &str = "compatible";

/// `TransitEncryptionMode`'s values (`smithy_query(query="shape",
/// shape="TransitEncryptionMode")`: `preferred | required`). "Setting
/// TransitEncryptionMode to required is a two-step process that requires you to
/// first set the TransitEncryptionMode to preferred."
pub const TRANSIT_ENCRYPTION_PREFERRED: &str = "preferred";
/// See [`TRANSIT_ENCRYPTION_PREFERRED`].
pub const TRANSIT_ENCRYPTION_REQUIRED: &str = "required";

/// `AuthTokenUpdateStrategyType`'s values (`smithy_query(query="shape",
/// shape="AuthTokenUpdateStrategyType")`: `SET | ROTATE | DELETE`), with the
/// documented order between them: "ROTATE - default, if no update strategy is
/// provided. SET - allowed only after ROTATE. DELETE - allowed only when
/// transitioning to RBAC."
pub const AUTH_TOKEN_ROTATE: &str = "ROTATE";
/// See [`AUTH_TOKEN_ROTATE`].
pub const AUTH_TOKEN_SET: &str = "SET";
/// See [`AUTH_TOKEN_ROTATE`].
pub const AUTH_TOKEN_DELETE: &str = "DELETE";

/// `AuthTokenUpdateStatus`'s values (`smithy_query(query="shape",
/// shape="AuthTokenUpdateStatus")`: `SETTING | ROTATING`): what a DEFERRED
/// auth-token update of each strategy is reported as until it is applied.
pub const AUTH_TOKEN_STATUS_ROTATING: &str = "ROTATING";
/// See [`AUTH_TOKEN_STATUS_ROTATING`].
pub const AUTH_TOKEN_STATUS_SETTING: &str = "SETTING";

/// The longest `ReplicationGroupDescription` a replication group may carry, as
/// `ModifyReplicationGroup$ReplicationGroupDescription` documents it: "A
/// description for the replication group. Maximum length is 255 characters."
/// The member carries no modelled `@length`, so the bound is enforced in the
/// handler.
pub const MAX_REPLICATION_GROUP_DESCRIPTION_LENGTH: usize = 255;

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
    ///
    /// The ORDER is this emulator's own: `NodeGroupMembers` carries no modelled
    /// ordering, so a client must read a member's role from `CurrentRole`
    /// rather than from its position. Holding the primary first is what lets
    /// [`ComAmazonawsElasticacheAmazonelasticachev9State::set_shard_replica_count`]
    /// drop the newest replicas from the end while keeping the primary, and
    /// [`NodeGroupRecord::fail_over`] maintains it.
    pub members: Vec<NodeGroupMemberRecord>,
}

impl NodeGroupRecord {
    /// Fail this shard over: the first replica takes the primary role and the
    /// primary it had becomes a replica, which is what an automatic failover
    /// does to a shard. Hands back `(demoted cluster, promoted cluster)`, or
    /// `None` when the shard has no primary or no replica to promote.
    ///
    /// The promoted member is moved to the front, because this record holds its
    /// members "the primary first" and [`ReplicationGroupRecord`]'s replica
    /// scaling relies on that order.
    pub fn fail_over(&mut self) -> Option<(String, String)> {
        let primary = self
            .members
            .iter()
            .position(|member| member.current_role == ROLE_PRIMARY)?;
        let replica = self
            .members
            .iter()
            .position(|member| member.current_role != ROLE_PRIMARY)?;
        self.swap_roles(primary, replica)
    }

    /// Promote the member cluster `cache_cluster_id` of this shard to the
    /// primary role, demoting the primary it had — which is what
    /// `ModifyReplicationGroup$PrimaryClusterId` asks for. Hands back
    /// `(demoted cluster, promoted cluster)`, or `None` when this shard has no
    /// such replica, or no primary to demote.
    ///
    /// Promotion by NAME and [`Self::fail_over`] are the same move over this
    /// shard's members, so they go through the one implementation below rather
    /// than two copies that can disagree about the member order.
    pub fn promote(&mut self, cache_cluster_id: &str) -> Option<(String, String)> {
        let primary = self
            .members
            .iter()
            .position(|member| member.current_role == ROLE_PRIMARY)?;
        let replica = self.members.iter().position(|member| {
            member.current_role != ROLE_PRIMARY && member.cache_cluster_id == cache_cluster_id
        })?;
        self.swap_roles(primary, replica)
    }

    /// Give the member at `replica` the primary role and the one at `primary`
    /// the replica role, keeping the primary at the front of `members`.
    fn swap_roles(&mut self, primary: usize, replica: usize) -> Option<(String, String)> {
        self.members[primary].current_role = ROLE_REPLICA.to_string();
        self.members[replica].current_role = ROLE_PRIMARY.to_string();
        let demoted = self.members[primary].cache_cluster_id.clone();
        let promoted = self.members[replica].cache_cluster_id.clone();
        let promoted_member = self.members.remove(replica);
        self.members.insert(0, promoted_member);
        Some((demoted, promoted))
    }

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

/// The changes a `ModifyReplicationGroup` deferred past this call: the members
/// the modelled `ReplicationGroupPendingModifiedValues` carries, which are
/// exactly the ones `ApplyImmediately` governs. A member that shape does NOT
/// model (the description, the maintenance window, the snapshot settings, the
/// node type) has nowhere to be reported as pending, so it is applied at once.
#[derive(Debug, Clone, Default)]
pub struct ReplicationGroupPendingValues {
    pub primary_cluster_id: Option<String>,
    /// One of `PendingAutomaticFailoverStatus`'s values (`enabled |
    /// disabled`).
    pub automatic_failover_status: Option<String>,
    /// One of `AuthTokenUpdateStatus`'s values (`SETTING | ROTATING`).
    pub auth_token_status: Option<String>,
    pub user_group_ids_to_add: Vec<String>,
    pub user_group_ids_to_remove: Vec<String>,
    pub log_delivery_configurations: Vec<LogDeliveryRecord>,
    pub transit_encryption_enabled: Option<bool>,
    /// One of `TransitEncryptionMode`'s values (`preferred | required`).
    pub transit_encryption_mode: Option<String>,
    /// One of `ClusterMode`'s values (`enabled | disabled | compatible`).
    pub cluster_mode: Option<String>,
}

impl ReplicationGroupPendingValues {
    /// True when nothing is pending.
    pub fn is_empty(&self) -> bool {
        self.primary_cluster_id.is_none()
            && self.automatic_failover_status.is_none()
            && self.auth_token_status.is_none()
            && self.user_group_ids_to_add.is_empty()
            && self.user_group_ids_to_remove.is_empty()
            && self.log_delivery_configurations.is_empty()
            && self.transit_encryption_enabled.is_none()
            && self.transit_encryption_mode.is_none()
            && self.cluster_mode.is_none()
    }

    /// These values as the modelled shape, or `None` when nothing is pending —
    /// a group with no deferred change reports no member rather than an empty
    /// structure.
    pub fn to_modeled(&self) -> Option<ReplicationGroupPendingModifiedValues> {
        if self.is_empty() {
            return None;
        }
        Some(ReplicationGroupPendingModifiedValues {
            primary_cluster_id: self.primary_cluster_id.clone(),
            automatic_failover_status: self.automatic_failover_status.clone(),
            // An online resharding is `ModifyReplicationGroupShardConfiguration`'s
            // to report, and that operation is a scaffolded 501 with its own
            // issue; nothing this operation defers is a resharding.
            resharding: None,
            auth_token_status: self.auth_token_status.clone(),
            user_groups: (!self.user_group_ids_to_add.is_empty()
                || !self.user_group_ids_to_remove.is_empty())
            .then(|| UserGroupsUpdateStatus {
                user_group_ids_to_add: (!self.user_group_ids_to_add.is_empty())
                    .then(|| self.user_group_ids_to_add.clone()),
                user_group_ids_to_remove: (!self.user_group_ids_to_remove.is_empty())
                    .then(|| self.user_group_ids_to_remove.clone()),
            }),
            log_delivery_configurations: (!self.log_delivery_configurations.is_empty()).then(|| {
                self.log_delivery_configurations
                    .iter()
                    .map(|record| PendingLogDeliveryConfiguration {
                        log_type: record.log_type.clone(),
                        destination_type: record.destination_type.clone(),
                        destination_details: record.destination_details.clone(),
                        log_format: record.log_format.clone(),
                    })
                    .collect()
            }),
            transit_encryption_enabled: self.transit_encryption_enabled,
            transit_encryption_mode: self.transit_encryption_mode.clone(),
            cluster_mode: self.cluster_mode.clone(),
        })
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
    /// The change `ModifyReplicationGroup` deferred past the call, which is
    /// what `ReplicationGroup$PendingModifiedValues` reports; see
    /// [`ReplicationGroupPendingValues`].
    pub pending_modified_values: ReplicationGroupPendingValues,
    /// "The cluster ID that is used as the daily snapshot source for the
    /// replication group", as `ModifyReplicationGroup$SnapshottingClusterId`
    /// set it.
    pub snapshotting_cluster_id: Option<String>,
    /// The `AuthTokenUpdateStrategyType` the last accepted auth-token update
    /// used, which is what makes the documented `SET` step reachable: "SET -
    /// allowed only after ROTATE".
    pub auth_token_last_strategy: Option<String>,
    /// The open data migration INTO this group, which is what refuses a second
    /// start on it and what `CompleteMigration` finishes; see
    /// [`MigrationRecord`]. `ReplicationGroup` models no member for it, so it is
    /// state only. `None` until a migration is opened on it.
    pub migration: Option<MigrationRecord>,
    /// The Global datastore this group is a member of, and its role in it,
    /// which is what `ReplicationGroup$GlobalReplicationGroupInfo` reports.
    /// `CreateGlobalReplicationGroup` writes them for the primary it builds a
    /// datastore around, and the Global datastore operations that MOVE a
    /// membership (associate, disassociate, failover, delete) write them for
    /// every other member.
    pub global_replication_group_id: Option<String>,
    /// See [`ReplicationGroupRecord::global_replication_group_id`].
    pub global_member_role: Option<String>,
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
    /// past its due instant, and hand back the change it made. See
    /// [`CacheClusterRecord::settle`]: this is the one write a reporting
    /// operation may make.
    pub fn settle(&mut self, now: SystemTime) -> Option<SettledTransition> {
        let pending = self.pending_status.as_ref()?;
        if now < pending.due {
            return None;
        }
        let settled = SettledTransition {
            due: pending.due,
            from: self.status.clone(),
            to: pending.status.clone(),
        };
        self.status = pending.status.clone();
        self.pending_status = None;
        for shard in &mut self.node_groups {
            shard.status = self.status.clone();
        }
        Some(settled)
    }

    /// How many replicas the shard `node_group_id` has: every member of it that
    /// is not the primary.
    pub fn shard_replica_count(&self, node_group_id: &str) -> Option<i32> {
        let shard = self
            .node_groups
            .iter()
            .find(|shard| shard.node_group_id == node_group_id)?;
        Some(
            i32::try_from(
                shard
                    .members
                    .iter()
                    .filter(|member| member.current_role != ROLE_PRIMARY)
                    .count(),
            )
            .unwrap_or(0),
        )
    }

    /// Whether `cache_cluster_id` is a REPLICA of the shard `node_group_id`:
    /// what `ReplicasToRemove` has to name.
    pub fn is_shard_replica(&self, node_group_id: &str, cache_cluster_id: &str) -> bool {
        self.node_groups
            .iter()
            .filter(|shard| shard.node_group_id == node_group_id)
            .flat_map(|shard| shard.members.iter())
            .any(|member| {
                member.current_role != ROLE_PRIMARY
                    && member.cache_cluster_id == cache_cluster_id
            })
    }

    /// Whether `cache_cluster_id` is a member cluster of this group — what
    /// `PrimaryClusterId` and `SnapshottingClusterId` have to name.
    pub fn is_member_cluster(&self, cache_cluster_id: &str) -> bool {
        self.node_groups
            .iter()
            .flat_map(|shard| shard.members.iter())
            .any(|member| member.cache_cluster_id == cache_cluster_id)
    }

    /// Promote the member cluster `cache_cluster_id` to the primary role of
    /// the shard it belongs to, demoting that shard's current primary — which
    /// is what `ModifyReplicationGroup$PrimaryClusterId` asks for. Hands back
    /// `(demoted cluster, promoted cluster)`, or `None` when no shard of this
    /// group holds it as a replica (a cluster that is already the primary is
    /// such a case: there is nothing to move).
    pub fn promote_primary(&mut self, cache_cluster_id: &str) -> Option<(String, String)> {
        self.node_groups
            .iter_mut()
            .find_map(|shard| shard.promote(cache_cluster_id))
    }

    /// Start the span after which the clock owes this group `available`, leaving
    /// it and its shards in `status` until then — the same transition a create
    /// gets, so a caller that polls straight after a change sees the transient
    /// status.
    pub fn begin_transition(&mut self, status: &str, now: SystemTime) {
        self.status = status.to_string();
        self.pending_status = Some(PendingTransition {
            due: now + CLUSTER_TRANSITION,
            status: STATUS_AVAILABLE.to_string(),
        });
        for shard in &mut self.node_groups {
            shard.status = status.to_string();
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
            // The Global datastore this group is a member of, if the Global
            // datastore operations have made it one; see
            // [`ReplicationGroupRecord::global_replication_group_id`].
            global_replication_group_info: self.global_replication_group_id.as_ref().map(|id| {
                GlobalReplicationGroupInfo {
                    global_replication_group_id: Some(id.clone()),
                    global_replication_group_member_role: self.global_member_role.clone(),
                }
            }),
            status: Some(self.status.clone()),
            pending_modified_values: self.pending_modified_values.to_modeled(),
            member_clusters: Some(self.member_clusters()),
            node_groups: Some(
                self.node_groups
                    .iter()
                    .map(|shard| shard.to_modeled(self, clusters))
                    .collect(),
            ),
            // The daily snapshot source, as `ModifyReplicationGroup` set it.
            snapshotting_cluster_id: self.snapshotting_cluster_id.clone(),
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
    /// The VPC the group's subnets belong to: the VPC of the subnets the group
    /// was created with, which is the account's default VPC in that Region
    /// ([`default_vpc_id`]) because every subnet [`region_subnets`] reports
    /// belongs to it.
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
pub const CACHE_ENGINE_VERSIONS: [PublishedEngineVersion; 32] = [
    // The oldest Memcached version ElastiCache still offers, and the version
    // `EngineVersion`'s own documentation uses as its example.
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
/// The catalog describes an engine by the engine's OWN published name, spelled
/// the way that engine spells it rather than re-cased by this service:
/// `memcached` writes its name in lower case, `Redis` and `Valkey` capitalise
/// theirs. That spelling is also what
/// `CacheEngineVersion$CacheEngineVersionDescription` is built from (see
/// [`published_cache_engine_versions`]), so a client selecting an engine by
/// matching either description string finds the entry under one name.
pub fn engine_description(engine: &str) -> &'static str {
    match engine {
        "memcached" => "memcached",
        ENGINE_VALKEY => "Valkey",
        _ => "Redis",
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
/// rather than an enum, so these are the spellings.
pub const SERVERLESS_STATUS_CREATING: &str = "CREATING";
/// See [`SERVERLESS_STATUS_CREATING`]. What a cache answers while ElastiCache
/// works a `ModifyServerlessCache` through
/// ([`ServerlessCacheRecord::begin_modification`]).
pub const SERVERLESS_STATUS_MODIFYING: &str = "MODIFYING";
/// See [`SERVERLESS_STATUS_CREATING`].
pub const SERVERLESS_STATUS_AVAILABLE: &str = "AVAILABLE";
/// See [`SERVERLESS_STATUS_CREATING`]. What a cache answers between the
/// `DeleteServerlessCache` that started its deletion and the removal the clock
/// owes it ([`ServerlessCacheRecord::pending_deletion`]).
pub const SERVERLESS_STATUS_DELETING: &str = "DELETING";

/// How long a serverless cache stays `CREATING` (or `MODIFYING`) before the
/// clock owes it `AVAILABLE`, and how long one stays `DELETING` before the clock
/// owes its removal — the same span a cache cluster's transitions take; see
/// [`CLUSTER_TRANSITION`] for why it is a chosen, non-zero span.
pub const SERVERLESS_CACHE_TRANSITION: Duration = CLUSTER_TRANSITION;

/// How many serverless caches the store holds before a create is refused with
/// `ServerlessCacheQuotaForCustomerExceededFault`. The emulator's OWN bound on
/// the store, for the reason [`CLUSTER_QUOTA`] gives.
pub const SERVERLESS_CACHE_QUOTA: usize = 150;

/// How many serverless cache snapshots the store holds before a create is
/// refused with `ServerlessCacheSnapshotQuotaExceededFault`. The emulator's OWN
/// bound on the store, for the reason [`CLUSTER_QUOTA`] gives.
pub const SERVERLESS_CACHE_SNAPSHOT_QUOTA: usize = 300;

/// What `ServerlessCacheSnapshot$SnapshotType` reports for a snapshot a
/// `CreateServerlessCacheSnapshot` took.
///
/// The member is "the type of snapshot of serverless cache" and carries no
/// modelled enum, and a snapshot a caller asked for by name is a manual one —
/// the word `Snapshot$SnapshotSource` reports a manually created snapshot under
/// as well ([`SNAPSHOT_SOURCE_MANUAL`]). No automatic snapshot is ever stored,
/// for the reason that constant gives: ElastiCache would take one during the
/// source cache's `DailySnapshotTime`, a day after the cache was created at the
/// earliest.
pub const SERVERLESS_SNAPSHOT_TYPE_MANUAL: &str = SNAPSHOT_SOURCE_MANUAL;

/// The page a `DescribeServerlessCacheSnapshots` serves when the request names
/// no `MaxResults`, and the largest page it serves at all: "The default is 50.
/// The Validation Constraints are a maximum of 50."
/// (`smithy_query(query="doc", shape="DescribeServerlessCacheSnapshotsRequest",
/// member="MaxResults")`).
pub const SERVERLESS_SNAPSHOT_MAX_RESULTS_DEFAULT: i32 = 50;
/// See [`SERVERLESS_SNAPSHOT_MAX_RESULTS_DEFAULT`].
pub const SERVERLESS_SNAPSHOT_MAX_RESULTS_MAX: i32 = 50;

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

/// What a request naming a `UserGroupId` for a Memcached serverless cache is
/// refused with: a user group is "available for Valkey and Redis OSS only"
/// (`CreateServerlessCache$UserGroupId`), so the combination is incompatible
/// rather than a missing group. One sentence for every handler that raises it.
pub const USER_GROUP_ENGINE_UNSUPPORTED: &str =
    "UserGroupId is only valid for a serverless cache running valkey or redis, not for memcached.";

/// The longest `Description` a serverless cache may carry, as
/// `CreateServerlessCache$Description` documents it: "The maximum length is 255
/// characters." The member carries no modelled `@length`, so the bound is
/// enforced in the handler.
pub const MAX_SERVERLESS_DESCRIPTION_LENGTH: usize = 255;

/// What ElastiCache reports for a resource whose description is BLANK.
///
/// The API normalises a blank description to a single space, whichever resource
/// carries it: a recorded `CreateServerlessCache` that names no `Description` is
/// answered with `" "` (the member documents "The default is NULL, i.e. if no
/// description is provided then an empty string will be returned"), and a
/// recorded `CreateCacheParameterGroup` that sends `Description=''` is reported
/// by `DescribeCacheParameterGroups` as `" "` too. The normalised value is
/// STORED, so a create response and a later describe cannot disagree.
pub const BLANK_DESCRIPTION: &str = " ";

/// See [`BLANK_DESCRIPTION`]: what a serverless cache created without a
/// description reports.
pub const DEFAULT_SERVERLESS_DESCRIPTION: &str = BLANK_DESCRIPTION;

/// The description a resource carries for the value `requested` named: the value
/// itself, byte for byte, or [`BLANK_DESCRIPTION`] when it is blank.
///
/// One helper, so every resource whose description a request SETS — the cache
/// parameter group, the cache subnet group, the replication group and the Global
/// datastore — normalises it the one way the service does, and a client that
/// round-trips descriptions reads back a value the service would answer.
pub fn stored_description(requested: &str) -> String {
    if requested.trim().is_empty() {
        BLANK_DESCRIPTION.to_string()
    } else {
        requested.to_string()
    }
}

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

/// The ARN of a serverless cache snapshot:
/// `arn:aws:elasticache:<region>:<account>:serverlesscachesnapshot:<name>`.
pub fn serverless_cache_snapshot_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("serverlesscachesnapshot:{name}"),
    )
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

/// Why a serverless cache cannot carry the `Description` a request named, or
/// the description it carries for that value.
///
/// The member carries no modelled `@length`, so the documented bound
/// ([`MAX_SERVERLESS_DESCRIPTION_LENGTH`]) is enforced here and a longer value
/// is refused rather than stored truncated; a blank one is the empty
/// description the API reports as [`DEFAULT_SERVERLESS_DESCRIPTION`]. One
/// implementation, so a created and a modified cache describe the same values.
pub fn serverless_description(requested: &str) -> Result<String, String> {
    let length = requested.chars().count();
    if length > MAX_SERVERLESS_DESCRIPTION_LENGTH {
        return Err(format!(
            "Invalid value for Description: the maximum length is \
             {MAX_SERVERLESS_DESCRIPTION_LENGTH} characters, got {length}."
        ));
    }
    Ok(stored_description(requested))
}

/// Why a serverless cache cannot carry the `SnapshotRetentionLimit` a request
/// named, or `None` when it may.
///
/// "The maximum value allowed is 35 days" ([`MAX_SNAPSHOT_RETENTION_LIMIT`]),
/// and a negative retention retains nothing: a limit outside that range is
/// refused rather than clamped.
pub fn snapshot_retention_limit_refusal(limit: i32) -> Option<String> {
    (!(0..=MAX_SNAPSHOT_RETENTION_LIMIT).contains(&limit)).then(|| {
        format!(
            "Invalid value {limit} for SnapshotRetentionLimit: must be at least 0 and no more \
             than {MAX_SNAPSHOT_RETENTION_LIMIT} days."
        )
    })
}

/// Why a serverless cache cannot carry the `DailySnapshotTime` a request named,
/// or `None` when it may; see [`is_daily_snapshot_time`].
pub fn daily_snapshot_time_refusal(requested: &str) -> Option<String> {
    (!is_daily_snapshot_time(requested)).then(|| {
        format!(
            "Invalid value {requested} for DailySnapshotTime: must be a daily time in the format \
             HH:MM (UTC)."
        )
    })
}

/// Why a serverless cache running `engine` cannot carry the
/// `MajorEngineVersion` a request named, or `None` when it may; see
/// [`is_version_label`].
pub fn major_engine_version_refusal(engine: &str, requested: &str) -> Option<String> {
    (!is_version_label(requested)).then(|| {
        format!(
            "Invalid value {requested} for MajorEngineVersion: must be a version such as {}.",
            default_major_engine_version(engine)
        )
    })
}

/// The `CacheUsageLimits` a cache carries after a request that named
/// `requested`, over the `stored` limits it carries now.
///
/// A limit is a BOUND rather than a document: each bound the request names
/// replaces the stored one, and a bound it does not name keeps the value the
/// cache is running — which is why the API publishes [`USAGE_LIMIT_REMOVED`] as
/// the way to remove a bound rather than treating an omitted member as a
/// removal. The result is what [`validate_cache_usage_limits`] is checked
/// against, so a request that lifts one bound past the other's stored value is
/// refused on the limits the cache would END UP with.
pub fn merge_cache_usage_limits(
    stored: Option<&CacheUsageLimits>,
    requested: &CacheUsageLimits,
) -> CacheUsageLimits {
    let stored_storage = stored.and_then(|limits| limits.data_storage.as_ref());
    let stored_ecpu = stored.and_then(|limits| limits.ecpu_per_second.as_ref());
    CacheUsageLimits {
        data_storage: match (requested.data_storage.as_ref(), stored_storage) {
            (Some(wanted), stored) => Some(DataStorage {
                minimum: wanted.minimum.or_else(|| stored.and_then(|s| s.minimum)),
                maximum: wanted.maximum.or_else(|| stored.and_then(|s| s.maximum)),
                // `DataStorage$Unit` is required and has one value (`GB`), so a
                // named member always carries it; an empty one keeps the unit
                // the cache is measured in.
                unit: if wanted.unit.is_empty() {
                    stored.map(|s| s.unit.clone()).unwrap_or_default()
                } else {
                    wanted.unit.clone()
                },
            }),
            (None, stored) => stored.cloned(),
        },
        ecpu_per_second: match (requested.ecpu_per_second.as_ref(), stored_ecpu) {
            (Some(wanted), stored) => Some(ECPUPerSecond {
                minimum: wanted.minimum.or_else(|| stored.and_then(|s| s.minimum)),
                maximum: wanted.maximum.or_else(|| stored.and_then(|s| s.maximum)),
            }),
            (None, stored) => stored.cloned(),
        },
    }
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
                // The template states the RULE the value broke — the member, its
                // bounds and the value that removes the limit — and carries
                // nothing of the value that broke it.
                return Err(match upper {
                    Some(upper) => format!(
                        "Invalid value for {member} limit. Must be between \
                         {DATA_STORAGE_MIN_GB} and {upper}. You can set the value to zero to \
                         remove the limit."
                    ),
                    None => format!(
                        "Invalid value for {member} limit. Must be at least \
                         {DATA_STORAGE_MIN_GB}. You can set the value to zero to remove the \
                         limit."
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

/// `MigrationSummary$Status` while the migration's resources are set up and no
/// data is moving yet: what `PrepareMigration` ("prepares the serverless cache
/// for data migration by validating the configuration and setting up the
/// migration resources") leaves behind.
pub const MIGRATION_STATUS_PREPARED: &str = "prepared";

/// `MigrationSummary$Status` while data is being copied from the source: the
/// phase `StopMigration` is documented against ("stops an ongoing data sync
/// between the source and the serverless cache \u2026 valid only while the migration
/// is syncing").
pub const MIGRATION_STATUS_SYNCING: &str = "syncing";

/// `MigrationSummary$Status` after a `StopMigration`: "the migration remains
/// open and the data sync can be restarted".
pub const MIGRATION_STATUS_STOPPED: &str = "stopped";

/// `MigrationSummary$Status` while requests are being moved onto the target: the
/// phase `StopMigration` refuses in ("during cutover it returns
/// `InvalidServerlessCacheStateFault`").
pub const MIGRATION_STATUS_CUTOVER: &str = "cutover";

/// `MigrationSummary$CutoverMode` when ElastiCache cuts over by itself, which is
/// what `StartMigration$AutoCutover` asks for: "whether ElastiCache
/// automatically attempts to complete the migration after data sync is
/// established".
pub const CUTOVER_MODE_AUTOMATIC: &str = "automatic";

/// `MigrationSummary$CutoverMode` when the caller cuts over itself, with
/// `CompleteMigration`.
pub const CUTOVER_MODE_MANUAL: &str = "manual";

/// The widest TCP port an endpoint can name.
pub const MAX_PORT: i32 = 65_535;

/// The most source endpoints one migration reads from.
///
/// A migration reads one source per shard of the target's keyspace, and a
/// keyspace is partitioned across at most [`MAX_NODE_GROUPS`] shards, so a
/// longer list names no source a target could read from \u2014 which is also what
/// keeps the list a request stores on the migration record bounded.
pub const MAX_SOURCE_ENDPOINTS: usize = MAX_NODE_GROUPS as usize;

/// One endpoint a migration reads data FROM, as a request's
/// `CustomerNodeEndpoint` named it: "the endpoint from which data should be
/// migrated".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceEndpointRecord {
    pub address: String,
    pub port: i32,
}

/// One open data migration, held on the resource it migrates INTO.
///
/// A migration is one thing however it was targeted, so there is one record of
/// it: `ServerlessCache$Migration` reports the copy a serverless cache carries
/// ([`ServerlessCacheRecord::migration`]), and the copy a replication group
/// carries ([`ReplicationGroupRecord::migration`]) is what the group's
/// already-under-migration refusal reads \u2014 `ReplicationGroup` models no member
/// for it.
#[derive(Debug, Clone)]
pub struct MigrationRecord {
    /// One of [`MIGRATION_STATUS_PREPARED`], [`MIGRATION_STATUS_SYNCING`],
    /// [`MIGRATION_STATUS_STOPPED`] or [`MIGRATION_STATUS_CUTOVER`].
    pub status: String,
    /// The endpoints data is read from: for a replication group the ones
    /// `StartMigration` named, for a serverless cache the ones
    /// `PrepareMigration` did ("for serverless cache migrations, ElastiCache
    /// uses the endpoint that you provided in `PrepareMigration`").
    pub source_endpoints: Vec<SourceEndpointRecord>,
    /// [`CUTOVER_MODE_AUTOMATIC`] or [`CUTOVER_MODE_MANUAL`], once a
    /// `StartMigration` has said which; a migration that has only been prepared
    /// has not been told yet.
    pub cutover_mode: Option<String>,
    /// When the data sync last started, which is what makes an open migration
    /// datable in the event log.
    pub started_at: Option<SystemTime>,
}

impl MigrationRecord {
    /// The migration a `PrepareMigration` sets up: its resources are ready and
    /// no data is moving yet.
    pub fn prepared(source_endpoints: Vec<SourceEndpointRecord>) -> Self {
        Self {
            status: MIGRATION_STATUS_PREPARED.to_string(),
            source_endpoints,
            cutover_mode: None,
            started_at: None,
        }
    }

    /// Start (or restart) the data sync, recording which side carries out the
    /// cutover.
    pub fn begin_sync(&mut self, cutover_mode: &str, now: SystemTime) {
        self.status = MIGRATION_STATUS_SYNCING.to_string();
        self.cutover_mode = Some(cutover_mode.to_string());
        self.started_at = Some(now);
    }

    /// Whether a data sync can be started from this phase: a prepared migration
    /// has not synced yet and a stopped one "can be restarted", while one
    /// already syncing or in cutover is past that point.
    pub fn can_begin_sync(&self) -> bool {
        self.status == MIGRATION_STATUS_PREPARED || self.status == MIGRATION_STATUS_STOPPED
    }

    /// This record as the modelled `MigrationSummary`.
    ///
    /// `migration_endpoint` is "the standard endpoint of the serverless cache",
    /// which the cache holds once, so it is passed in rather than held a second
    /// time here.
    pub fn to_modeled(&self, migration_endpoint: Endpoint) -> MigrationSummary {
        MigrationSummary {
            status: Some(self.status.clone()),
            migration_endpoint: Some(migration_endpoint),
            cutover_mode: self.cutover_mode.clone(),
        }
    }
}

/// Validate a request's `CustomerNodeEndpointList` into the endpoints a
/// migration reads from, or answer the sentence the request is refused with.
///
/// The one place a migration's source endpoints are checked, so every migration
/// operation that takes the list ("the list of endpoints from which to migrate
/// data") holds it to the same rule: at least one endpoint, each with an address
/// and a port a TCP endpoint can carry.
pub fn source_endpoints(
    endpoints: &[CustomerNodeEndpoint],
) -> Result<Vec<SourceEndpointRecord>, String> {
    if endpoints.is_empty() {
        return Err(
            "The parameter CustomerNodeEndpointList must be provided and must not be blank."
                .to_string(),
        );
    }
    if endpoints.len() > MAX_SOURCE_ENDPOINTS {
        return Err(format!(
            "Invalid value for CustomerNodeEndpointList: a migration reads from at most \
             {MAX_SOURCE_ENDPOINTS} endpoints."
        ));
    }
    let mut out = Vec::with_capacity(endpoints.len());
    for endpoint in endpoints {
        let address = endpoint.address.as_deref().unwrap_or_default().trim();
        if address.is_empty() {
            return Err(
                "The parameter CustomerNodeEndpoint.Address must be provided and must not be \
                 blank."
                    .to_string(),
            );
        }
        let Some(port) = endpoint.port else {
            return Err(
                "The parameter CustomerNodeEndpoint.Port must be provided and must not be blank."
                    .to_string(),
            );
        };
        if !(1..=MAX_PORT).contains(&port) {
            return Err(format!(
                "Invalid value {port} for Port: must be between 1 and {MAX_PORT}."
            ));
        }
        out.push(SourceEndpointRecord {
            address: address.to_string(),
            port,
        });
    }
    Ok(out)
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
    /// The instant the clock owes this cache's REMOVAL from the store, set by
    /// the `DeleteServerlessCache` that put it in `DELETING`. A deletion is not
    /// instant at AWS either — the call answers with the cache reporting
    /// `DELETING` — so the record outlives the call by
    /// [`SERVERLESS_CACHE_TRANSITION`] and is swept by
    /// [`ComAmazonawsElasticacheAmazonelasticachev9State::purge_deleted_serverless_caches`].
    pub pending_deletion: Option<SystemTime>,
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
    /// The open data migration INTO this cache, which is the one copy
    /// `ServerlessCache$Migration` reports and every migration operation reads;
    /// see [`MigrationRecord`]. `None` until a migration is opened on it.
    pub migration: Option<MigrationRecord>,
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
    /// its due instant, and hand back the change it made; see
    /// [`CacheClusterRecord::settle`].
    pub fn settle(&mut self, now: SystemTime) -> Option<SettledTransition> {
        let pending = self.pending_status.as_ref()?;
        if now < pending.due {
            return None;
        }
        let settled = SettledTransition {
            due: pending.due,
            from: self.status.clone(),
            to: pending.status.clone(),
        };
        self.status = pending.status.clone();
        self.pending_status = None;
        Some(settled)
    }

    /// Put this cache into `MODIFYING` and record the `AVAILABLE` the clock now
    /// owes it, [`SERVERLESS_CACHE_TRANSITION`] from `now`.
    ///
    /// Scaling a cache's capacity, and moving it onto another engine or engine
    /// version, is work ElastiCache carries out after the call answers, so the
    /// cache is `MODIFYING` from here and a caller reading it straight after
    /// still sees that status and the state errors that go with it.
    pub fn begin_modification(&mut self, now: SystemTime) {
        self.status = SERVERLESS_STATUS_MODIFYING.to_string();
        self.pending_status = Some(PendingTransition {
            due: now + SERVERLESS_CACHE_TRANSITION,
            status: SERVERLESS_STATUS_AVAILABLE.to_string(),
        });
    }

    /// Put this cache into `DELETING` and record the removal the clock now owes
    /// it, [`SERVERLESS_CACHE_TRANSITION`] from `now`.
    ///
    /// The status change a delete makes is immediate (the call answers with the
    /// cache reporting `DELETING`), so it is NOT a pending transition; what the
    /// clock owes is the record's disappearance, which
    /// [`ComAmazonawsElasticacheAmazonelasticachev9State::purge_deleted_serverless_caches`]
    /// materialises. Any status change that was still pending is dropped: a
    /// cache on its way out is not owed `AVAILABLE` any more.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        self.status = SERVERLESS_STATUS_DELETING.to_string();
        self.pending_status = None;
        self.pending_deletion = Some(now + SERVERLESS_CACHE_TRANSITION);
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
            // "Provides a summary of the migration status for the serverless
            // cache": the open migration this cache carries, if one was opened
            // on it. Its endpoint is "the standard endpoint of the serverless
            // cache", which is the one this record already holds.
            migration: self.migration.as_ref().map(|migration| {
                migration.to_modeled(Endpoint {
                    address: Some(self.endpoint_address.clone()),
                    port: Some(self.endpoint_port),
                })
            }),
        }
    }
}

/// One serverless cache snapshot, as this emulator persists it: the source
/// cache's configuration as it stood when the snapshot was taken, plus the
/// snapshot's own identity and status.
///
/// "This API creates a copy of an entire ServerlessCache at a specific moment in
/// time", so the configuration below is COPIED out of the source cache record
/// when the snapshot is taken: a later `ModifyServerlessCache` cannot change
/// what a snapshot already taken reports, and a `DeleteServerlessCache` cannot
/// take it away with the cache.
#[derive(Debug, Clone)]
pub struct ServerlessCacheSnapshotRecord {
    /// The snapshot's identity, lowercased —
    /// `CreateServerlessCacheSnapshot$ServerlessCacheSnapshotName` "is stored as
    /// a lowercase string".
    pub name: String,
    pub arn: String,
    /// "The ID of the Amazon Web Services Key Management Service (KMS) key of a
    /// serverless cache snapshot", resolved to the key's own ARN when the
    /// request named one and otherwise the source cache's key.
    pub kms_key_id: Option<String>,
    /// [`SERVERLESS_SNAPSHOT_TYPE_MANUAL`].
    pub snapshot_type: String,
    /// The snapshot's current status, spelled as [`STATUS_CREATING`] /
    /// [`STATUS_AVAILABLE`] do.
    pub status: String,
    /// The status change the clock owes this snapshot, if any.
    pub pending_status: Option<PendingTransition>,
    /// "The date and time that the source serverless cache's metadata and cache
    /// data set was obtained for the snapshot."
    pub created_at: SystemTime,
    /// The source cache's name, engine and major engine version, which together
    /// are the `ServerlessCacheConfiguration` the snapshot reports.
    pub serverless_cache_name: String,
    pub engine: String,
    pub major_engine_version: String,
    /// The tags the create was given. The ONE copy of this snapshot's tags; see
    /// [`SnapshotRecord::tags`].
    pub tags: Vec<(String, String)>,
}

impl ServerlessCacheSnapshotRecord {
    /// A snapshot of `cache` as it stands now.
    ///
    /// A created snapshot is NOT ready: it answers [`STATUS_CREATING`], and the
    /// clock owes it [`STATUS_AVAILABLE`] once [`SNAPSHOT_TRANSITION`] has
    /// passed, so a caller reading it straight after the call still sees the
    /// transient status.
    pub fn from_cache(cache: &ServerlessCacheRecord, new: NewServerlessCacheSnapshot) -> Self {
        Self {
            arn: serverless_cache_snapshot_arn(&cache.region, &new.name),
            name: new.name,
            // "The ID of the KMS key used to encrypt the snapshot. Default:
            // NULL": a snapshot of a cache encrypted with a customer managed key
            // keeps that key when the request names none, the same way
            // [`SnapshotRecord::copy_to`] keeps the source snapshot's.
            kms_key_id: new.kms_key_id.or_else(|| cache.kms_key_id.clone()),
            snapshot_type: SERVERLESS_SNAPSHOT_TYPE_MANUAL.to_string(),
            status: STATUS_CREATING.to_string(),
            pending_status: Some(PendingTransition {
                due: new.now + SNAPSHOT_TRANSITION,
                status: STATUS_AVAILABLE.to_string(),
            }),
            created_at: new.now,
            serverless_cache_name: cache.name.clone(),
            engine: cache.engine.clone(),
            major_engine_version: cache.major_engine_version.clone(),
            tags: new.tags,
        }
    }

    /// Materialise the status change the clock owes this snapshot, if `now` is
    /// past its due instant, and hand back the change it made. The one write a
    /// reporting operation may make; see [`CacheClusterRecord::settle`].
    pub fn settle(&mut self, now: SystemTime) -> Option<SettledTransition> {
        let pending = self.pending_status.as_ref()?;
        if now < pending.due {
            return None;
        }
        let settled = SettledTransition {
            due: pending.due,
            from: self.status.clone(),
            to: pending.status.clone(),
        };
        self.status = pending.status.clone();
        self.pending_status = None;
        Some(settled)
    }

    /// This record as the modelled `ServerlessCacheSnapshot`.
    pub fn to_modeled(&self) -> ServerlessCacheSnapshot {
        ServerlessCacheSnapshot {
            serverless_cache_snapshot_name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            kms_key_id: self.kms_key_id.clone(),
            snapshot_type: Some(self.snapshot_type.clone()),
            status: Some(self.status.clone()),
            create_time: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            // "The time that the serverless cache snapshot will expire": only a
            // snapshot ElastiCache took automatically is kept for a retention
            // limit and so has an instant it expires at, and none is ever stored
            // (see [`SERVERLESS_SNAPSHOT_TYPE_MANUAL`]).
            expiry_time: None,
            // "The total size of a serverless cache snapshot, in bytes": this
            // emulator serves no cache data plane, so it holds no data set whose
            // size it could report rather than invent — the same reason
            // [`NodeSnapshotRecord::to_modeled`] reports no cache size.
            bytes_used_for_cache: None,
            serverless_cache_configuration: Some(ServerlessCacheConfiguration {
                serverless_cache_name: Some(self.serverless_cache_name.clone()),
                engine: Some(self.engine.clone()),
                major_engine_version: Some(self.major_engine_version.clone()),
            }),
        }
    }
}

/// The validated values a new serverless cache snapshot is built from; see
/// [`ServerlessCacheSnapshotRecord::from_cache`].
#[derive(Debug, Clone)]
pub struct NewServerlessCacheSnapshot {
    /// The stored (lowercased) `ServerlessCacheSnapshotName`.
    pub name: String,
    pub kms_key_id: Option<String>,
    pub tags: Vec<(String, String)>,
    pub now: SystemTime,
}

/// Why a serverless cache snapshot cannot be stored under a name, with the
/// sentence it is refused in; see
/// [`ComAmazonawsElasticacheAmazonelasticachev9State::serverless_cache_snapshot_refusal`].
///
/// The kind decides which modelled failure the MINTING operation lifts it into:
/// `CreateServerlessCacheSnapshot` models a fault for each, while
/// `DeleteServerlessCache` models only the already-exists one, so its
/// `FinalSnapshotName` reports the bound as the value it cannot accept.
#[derive(Debug, Clone)]
pub enum ServerlessSnapshotRefusal {
    /// A name another snapshot already holds — "must be unique for the customer
    /// account".
    AlreadyExists(String),
    /// The store is at [`SERVERLESS_CACHE_SNAPSHOT_QUOTA`].
    QuotaExceeded(String),
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
/// An entry's two descriptions come from two different names: the ENGINE's own
/// product name describes the engine ([`engine_description`]), and the engine's
/// API name — the `Engine` value itself — is what the version description is
/// built from, `<engine> version <version>`.
///
/// A `BTreeMap` is what gives `DescribeCacheEngineVersions` the stable total
/// order its `Marker` indexes into, and the per-engine order its `DefaultOnly`
/// reads the newest version off.
pub fn published_cache_engine_versions() -> BTreeMap<String, CacheEngineVersionRecord> {
    CACHE_ENGINE_VERSIONS
        .iter()
        .map(|row| {
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
                    cache_engine_description: engine_description(row.engine).to_string(),
                    cache_engine_version_description: format!(
                        "{} version {}",
                        row.engine, row.engine_version
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
    /// When the clock owes this snapshot's REMOVAL, set by `DeleteSnapshot`:
    /// "ElastiCache immediately begins deleting the snapshot", so the record
    /// answers [`STATUS_DELETING`] from that call and is dropped once the span
    /// has passed. `None` on a snapshot nobody has deleted.
    pub pending_removal: Option<SystemTime>,
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
    /// The tags `CreateSnapshot` was given. The ONE copy of this snapshot's tags:
    /// `ListTagsForResource` reports it and `AddTagsToResource` /
    /// `RemoveTagsFromResource` change it, through
    /// [`ComAmazonawsElasticacheAmazonelasticachev9State::resource_tags`].
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
            pending_removal: None,
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

    /// A copy of this snapshot stored under another name.
    ///
    /// A copy is "a copy of an existing snapshot", so everything that describes
    /// WHAT was captured — the source cluster's id and configuration, the
    /// instant the data was taken at, and the per-node snapshots — is carried
    /// over unchanged rather than re-read from the live cluster or re-stamped
    /// with the copy's own clock: copying does not retake the snapshot. Only the
    /// copy's own identity (name and ARN), its fresh status, its provenance and
    /// the members the copy request itself carries (KMS key, tags) differ.
    ///
    /// A copy is always a MANUAL snapshot, whatever the source's
    /// `SnapshotSource` was: copying an automated backup is how the ElastiCache
    /// User Guide has a caller keep it past its retention limit, and a manual
    /// snapshot is the one that "does not expire".
    pub fn copy_to(&self, new: SnapshotCopy) -> SnapshotRecord {
        SnapshotRecord {
            arn: snapshot_arn(&new.region, &new.target_snapshot_name),
            snapshot_name: new.target_snapshot_name,
            status: STATUS_CREATING.to_string(),
            pending_status: Some(PendingTransition {
                due: new.now + SNAPSHOT_TRANSITION,
                status: STATUS_AVAILABLE.to_string(),
            }),
            // The copy is its own resource: whatever the source's deletion
            // owes, nothing is owed on the record stored under the new name.
            pending_removal: None,
            source: SNAPSHOT_SOURCE_MANUAL.to_string(),
            // "The ID of the KMS key used to encrypt the target snapshot": the
            // copy keeps the source's key when the request names none.
            kms_key_id: new.kms_key_id.or_else(|| self.kms_key_id.clone()),
            // `Tags` are "a list of tags to be added to this resource" — the
            // copy's own, not the source's.
            tags: new.tags,
            ..self.clone()
        }
    }

    /// Materialise the status change the clock owes this snapshot, if `now` is
    /// past its due instant, and hand back the change it made. The one write a
    /// reporting operation may make; see [`CacheClusterRecord::settle`].
    pub fn settle(&mut self, now: SystemTime) -> Option<SettledTransition> {
        let pending = self.pending_status.as_ref()?;
        if now < pending.due {
            return None;
        }
        let settled = SettledTransition {
            due: pending.due,
            from: self.status.clone(),
            to: pending.status.clone(),
        };
        self.status = pending.status.clone();
        self.pending_status = None;
        Some(settled)
    }

    /// Put this snapshot into [`STATUS_DELETING`] and let the clock owe its
    /// removal [`SNAPSHOT_TRANSITION`] later.
    ///
    /// `DeleteSnapshot` documents exactly this: "When you receive a successful
    /// response from this operation, ElastiCache immediately begins deleting the
    /// snapshot; you cannot cancel or revert this operation" — so the record is
    /// still there, reporting `deleting`, and the removal is what the clock
    /// materialises ([`ComAmazonawsElasticacheAmazonelasticachev9State::purge_deleted_snapshots`]).
    /// Any status change that was still pending is dropped: a snapshot that is
    /// going away is not owed `available` any more.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        self.status = STATUS_DELETING.to_string();
        self.pending_status = None;
        self.pending_removal = Some(now + SNAPSHOT_TRANSITION);
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
    /// The tail of this row's `ServiceUpdateName`, FIXED by the catalog: an
    /// update's identity is part of what the service publishes, so it is the
    /// same string on every store this build brings up and is never derived
    /// from the clock — a client that discovered a name, or that was written
    /// against one, can still name it in `BatchApplyUpdateAction` /
    /// `BatchStopUpdateAction` a day later. See [`published_service_updates`]
    /// for the whole name.
    slug: &'static str,
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
        slug: "security-update-001",
        severity: "critical",
        released_days_before: 180,
        apply_by_days_after_release: 30,
        end_days_after_release: 120,
        auto_update_after_recommended_apply_by_date: true,
    },
    PublishedUpdate {
        slug: "security-update-002",
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

/// The values a copied snapshot carries that are not the source snapshot's —
/// one argument so [`SnapshotRecord::copy_to`] does not take a row of
/// positional strings.
#[derive(Debug, Clone)]
pub struct SnapshotCopy {
    /// `CopySnapshot$TargetSnapshotName`, already lowercased: "this value is
    /// stored as a lowercase string".
    pub target_snapshot_name: String,
    /// The region the copy's ARN names — the request's, as every other ARN this
    /// service mints.
    pub region: String,
    /// `CopySnapshot$KmsKeyId`, "the ID of the KMS key used to encrypt the
    /// target snapshot", or `None` to keep the source snapshot's.
    pub kms_key_id: Option<String>,
    pub tags: Vec<(String, String)>,
    pub now: SystemTime,
}

/// The service-update catalog as of `anchor`, keyed by `ServiceUpdateName`.
///
/// One update per engine per row of [`PUBLISHED_UPDATES`], named
/// `elasticache-<engine>-<slug>`: the service, the engine the update applies to
/// and the row's own [`PublishedUpdate::slug`]. The name is FIXED by the
/// catalog, so a store built at any instant publishes the same set of names and
/// a name a client holds stays resolvable; only the DATES hang off `anchor`,
/// which is what keeps the window shape a real account sees. A `BTreeMap` is
/// what gives `DescribeServiceUpdates` the stable total order its `Marker`
/// indexes into.
pub fn published_service_updates(anchor: SystemTime) -> BTreeMap<String, ServiceUpdateRecord> {
    let mut catalog = BTreeMap::new();
    for row in &PUBLISHED_UPDATES {
        let release_date = anchor
            .checked_sub(DAY * u32::try_from(row.released_days_before).unwrap_or(u32::MAX))
            .unwrap_or(anchor);
        for engine in ENGINES {
            let name = format!("{ARN_NAMESPACE}-{engine}-{}", row.slug);
            let engine_version = default_engine_version(engine);
            let severity = row.severity;
            catalog.insert(
                name.clone(),
                ServiceUpdateRecord {
                    service_update_name: name,
                    engine: engine.to_string(),
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

/// `UpdateActionStatus`'s values (`smithy_query(query="shape",
/// shape="UpdateActionStatus")`: `not-applied | waiting-to-start | in-progress
/// | stopping | stopped | complete | scheduling | scheduled |
/// not-applicable`).
///
/// `not-applied` is the status of an update action nothing has started, so no
/// record is stored for one: it is derived from the absence of a record rather
/// than kept. The four below it are the ones this slice moves through —
/// `BatchApplyUpdateAction` starts an action, `BatchStopUpdateAction` stops one
/// and the clock carries each transient status to the next. `scheduling` /
/// `scheduled` belong to applying an update at a chosen time, which no
/// operation in this API asks for, and `not-applicable` to an update that does
/// not apply to the resource at all.
pub const UPDATE_ACTION_NOT_APPLIED: &str = "not-applied";
/// See [`UPDATE_ACTION_NOT_APPLIED`].
pub const UPDATE_ACTION_WAITING_TO_START: &str = "waiting-to-start";
/// See [`UPDATE_ACTION_NOT_APPLIED`].
pub const UPDATE_ACTION_IN_PROGRESS: &str = "in-progress";
/// See [`UPDATE_ACTION_NOT_APPLIED`].
pub const UPDATE_ACTION_STOPPING: &str = "stopping";
/// See [`UPDATE_ACTION_NOT_APPLIED`].
pub const UPDATE_ACTION_STOPPED: &str = "stopped";
/// See [`UPDATE_ACTION_NOT_APPLIED`].
pub const UPDATE_ACTION_COMPLETE: &str = "complete";

/// How long an update action stays in each transient status before the clock
/// owes it the next one.
///
/// The same reasoning as [`CLUSTER_TRANSITION`]: a real service update takes
/// minutes, the emulator chooses a span, and it is deliberately NOT zero, so a
/// caller acting straight after `BatchApplyUpdateAction` still observes the
/// action under way — and can stop it. Nothing sleeps: the due instant is
/// stored on the record and materialised by whichever later call reads it.
pub const UPDATE_ACTION_TRANSITION: Duration = Duration::from_millis(500);

/// What `UnprocessedUpdateAction$ErrorType` carries when the resource an update
/// action was named for is not one this account holds.
///
/// `ErrorType` is a plain string with no modelled vocabulary, so these are the
/// names of the modelled fault shapes that refuse exactly those resources
/// elsewhere in this service, rather than a second spelling of the same
/// conditions.
pub const CACHE_CLUSTER_NOT_FOUND_ERROR_TYPE: &str = "CacheClusterNotFoundFault";
/// See [`CACHE_CLUSTER_NOT_FOUND_ERROR_TYPE`].
pub const REPLICATION_GROUP_NOT_FOUND_ERROR_TYPE: &str = "ReplicationGroupNotFoundFault";
/// See [`CACHE_CLUSTER_NOT_FOUND_ERROR_TYPE`]: what `ErrorType` carries when
/// the resource is one this account holds but cannot take the command — the
/// code of the only parameter-value error both batch operations model.
pub const UPDATE_ACTION_INVALID_ERROR_TYPE: &str = "InvalidParameterValue";

/// The status the clock owes an update action now in `status`, or `None` when
/// `status` is where the action stops.
fn next_update_action_status(status: &str) -> Option<&'static str> {
    match status {
        UPDATE_ACTION_WAITING_TO_START => Some(UPDATE_ACTION_IN_PROGRESS),
        UPDATE_ACTION_IN_PROGRESS => Some(UPDATE_ACTION_COMPLETE),
        UPDATE_ACTION_STOPPING => Some(UPDATE_ACTION_STOPPED),
        _ => None,
    }
}

/// True while an update action is still on its way through the update: the
/// statuses an apply does not start over, because work is already under way.
///
/// The statuses OUTSIDE this set — `not-applied`, `stopped` and `complete` —
/// all mean no work is under way on that resource, so an apply starts work from
/// any of them: applying a service update is a request to run it now, and
/// neither the model nor the ElastiCache documentation refuses one for a
/// resource whose previous run finished or was stopped. The stop path is the
/// mirror image and NOT symmetric for a reason: it needs work under way to act
/// on, which is why it refuses exactly those three statuses (see
/// [`update_action_stoppable`]).
fn update_action_in_flight(status: &str) -> bool {
    matches!(
        status,
        UPDATE_ACTION_WAITING_TO_START | UPDATE_ACTION_IN_PROGRESS | UPDATE_ACTION_STOPPING
    )
}

/// True when a stop has something left to stop: an action already `stopping` is
/// on its way to [`UPDATE_ACTION_STOPPED`], so stopping it again would move
/// nothing — and must not push the due instant of that status out.
fn update_action_stoppable(status: &str) -> bool {
    matches!(
        status,
        UPDATE_ACTION_WAITING_TO_START | UPDATE_ACTION_IN_PROGRESS
    )
}

/// True when an update action has finished running on its nodes: `complete`
/// (the update ran to the end) or `stopped` (it was stopped), the two statuses
/// the clock owes nothing beyond. These are the statuses a node reports an
/// update END date for — `NodeUpdateEndDate` is "the end date of the update for
/// a node", and an update still under way has none.
fn update_action_finished(status: &str) -> bool {
    matches!(status, UPDATE_ACTION_COMPLETE | UPDATE_ACTION_STOPPED)
}

/// `SlaMet`'s values (`smithy_query(query="shape", shape="SlaMet")`: `yes | no
/// | n/a`), as `UpdateAction$SlaMet` documents them: "If yes, all nodes in the
/// replication group have been updated by the recommended apply-by date. If no,
/// at least one node in the replication group have not been updated by the
/// recommended apply-by date."
///
/// `n/a` is the third value, and the answer while the question has none yet: the
/// recommended apply-by date has not passed and the update has not finished, so
/// no node is late.
pub const SLA_MET_YES: &str = "yes";
/// See [`SLA_MET_YES`].
pub const SLA_MET_NO: &str = "no";
/// See [`SLA_MET_YES`].
pub const SLA_MET_NOT_APPLICABLE: &str = "n/a";

/// `NodeUpdateInitiatedBy`'s values (`smithy_query(query="shape",
/// shape="NodeUpdateInitiatedBy")`: `system | customer`), which "reflects
/// whether the update was initiated by the customer or automatically applied".
///
/// Every update action this service starts was asked for by a
/// `BatchApplyUpdateAction` call, so `customer` is what it reports. `system`
/// belongs to the automatic apply after the recommended apply-by date, which no
/// operation in this API schedules and this emulator therefore never runs — an
/// action nothing applied stays [`UPDATE_ACTION_NOT_APPLIED`] however long the
/// clock carries it past that date, and reports no initiator at all.
pub const NODE_UPDATE_INITIATED_BY_CUSTOMER: &str = "customer";

/// Which of the two resource lists a batch update-action call named a resource
/// in: `ProcessedUpdateAction` / `UnprocessedUpdateAction` report a cache
/// cluster's id and a replication group's under different members, each kind is
/// resolved against its own state, and the kind is part of an update action's
/// stored key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum UpdateActionResource {
    /// A `CacheClusterIds` entry.
    CacheCluster,
    /// A `ReplicationGroupIds` entry.
    ReplicationGroup,
}

impl UpdateActionResource {
    /// How a message names this kind of resource.
    fn label(self) -> &'static str {
        match self {
            Self::CacheCluster => "cache cluster",
            Self::ReplicationGroup => "replication group",
        }
    }
}

// ---------------------------------------------------------------------------
// RBAC users
// ---------------------------------------------------------------------------

/// The cache engines an RBAC user may be created for, as `CreateUser$Engine`
/// documents it: "The options are valkey or redis". RBAC is a Valkey / Redis
/// OSS feature, so `memcached` — a legal CLUSTER engine, see [`ENGINES`] — is
/// not one of them.
pub const USER_ENGINES: [&str; 2] = ["redis", "valkey"];

/// How ElastiCache refuses an `Engine` that access control (RBAC) does not
/// exist for: the CAPABILITY rule, naming the engine version from which the
/// feature is available.
///
/// Wholly fixed text with no request value. The RBAC surface is "for Valkey
/// engine version 7.2 onwards and Redis OSS 6.0 to 7.1" (the user and
/// user-group operations' own documentation), so an engine outside it is
/// refused for lacking the feature rather than for being outside a vocabulary
/// the caller could pick another entry from.
pub const ACCESS_CONTROL_ENGINE_UNSUPPORTED: &str =
    "Access control options are available for Redis version 6.0 onwards.";

/// `engine` lowercased when it names an engine access control exists for, else
/// the capability refusal ([`ACCESS_CONTROL_ENGINE_UNSUPPORTED`]).
///
/// ElastiCache stores engine names in lowercase, so the comparison and the
/// stored value are both lowercase. One implementation, so every operation that
/// creates or changes a user or a user group answers the one sentence for an
/// engine the feature does not exist for; the set itself is [`USER_ENGINES`].
pub fn normalize_access_control_engine(engine: &str) -> Result<String, String> {
    let lowered = engine.to_ascii_lowercase();
    if USER_ENGINES.contains(&lowered.as_str()) {
        Ok(lowered)
    } else {
        Err(ACCESS_CONTROL_ENGINE_UNSUPPORTED.to_string())
    }
}

/// `engine` lowercased, or the reason it names no engine a Global datastore can
/// be moved to.
///
/// A Global datastore is "for Valkey or Redis OSS only", so the engine set is
/// the narrower one [`USER_ENGINES`] names; unlike the access-control surface
/// above, the rule here IS the vocabulary, so the refusal states it.
pub fn normalize_global_datastore_engine(engine: &str) -> Result<String, String> {
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

/// `User$Status` of a user `DeleteUser` has accepted: "Indicates the user status.
/// Can be \"active\", \"modifying\" or \"deleting\"". A delete answers with the
/// user in this status — it is the LAST thing this API reports about it, because
/// the clock then owes its removal from the store ([`USER_DELETION_TRANSITION`]),
/// after which the user is gone and every lookup of it answers the modelled
/// `UserNotFoundFault`.
pub const USER_STATUS_DELETING: &str = "deleting";

/// How long a user stays `deleting` before the clock owes its removal from the
/// store.
///
/// The same reasoning as [`CLUSTER_TRANSITION`]: ElastiCache takes its own time,
/// the emulator chooses a span, and it is deliberately NOT zero so a caller
/// acting straight after `DeleteUser` still observes the transient `deleting`
/// status and the state error that goes with it. The due instant is stored on the
/// record and materialised by whichever later call reads it — nothing sleeps.
pub const USER_DELETION_TRANSITION: Duration = CLUSTER_TRANSITION;

/// How many passwords one user may hold, as `CreateUser$Passwords` documents it:
/// "Passwords used for this user. You can create up to two passwords for each
/// user."
pub const MAX_PASSWORDS_PER_USER: usize = 2;

/// How many users the store holds before a create is refused with the modelled
/// `UserQuotaExceededFault`. The emulator's OWN bound on the store, for the
/// reason [`CLUSTER_QUOTA`] gives.
pub const USER_QUOTA: usize = 100;

/// The LONGEST `UserId` the service accepts: ElastiCache documents an RBAC user
/// id as 1 to 40 characters, so an id of exactly this length is ACCEPTED and
/// only a longer one is refused. The model constrains `UserId` with
/// `@length(min: 1)` and a pattern only, so the upper bound is enforced here, by
/// every operation that NAMES a user and can report the modelled
/// `InvalidParameterValueException` — before the id is looked up, because an id
/// the service would never have stored names no user to look up.
///
/// The refusal carries the service's own recorded wording, "User id length must
/// be less than 40.", which states the bound loosely; the BOUND is the
/// documented maximum rather than that sentence read literally.
pub const MAX_USER_ID_LEN: usize = 40;

/// Why `user_id` cannot name a user, or `None` when it can.
///
/// Stated once so `CreateUser` and `DeleteUser` cannot drift about which ids the
/// service accepts: a create that stores an id and a delete that refuses the
/// same one would leave a user nothing could remove.
pub fn user_id_rejection(user_id: &str) -> Option<String> {
    (user_id.chars().count() > MAX_USER_ID_LEN)
        .then(|| format!("User id length must be less than {MAX_USER_ID_LEN}."))
}

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

/// How ElastiCache words a `DefaultUserRequired` refusal — "You must add default
/// user to a user group."
///
/// One sentence for the whole error class, shared by every call that would leave
/// a user group without the built-in user ([`DEFAULT_USER_ID`]): it carries no
/// request-supplied value and no stored one, so there is nothing to interpolate
/// and no caller-visible wording to diverge between handlers. The HTTP status
/// (400) and the awsQuery code (`DefaultUserRequired`) are the modelled part of
/// the contract; this is the wording AWS was recorded answering with.
pub const DEFAULT_USER_REQUIRED_MESSAGE: &str =
    "Redis user group needs to contain a user with the user name default.";

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

/// Which batch operation is driving an update action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateActionCommand {
    /// `BatchApplyUpdateAction`: start the update on each named resource.
    Apply,
    /// `BatchStopUpdateAction`: stop the update under way on each named
    /// resource.
    Stop,
}

/// What a batch update-action call names: the service update, and the resources
/// it acts on.
///
/// `BatchApplyUpdateAction` and `BatchStopUpdateAction` model the same three
/// members, so both handlers hand the store this one request rather than
/// carrying two copies of the accounting below.
#[derive(Debug, Clone, Default)]
pub struct BatchUpdateActionRequest {
    /// "The unique ID of the service update", as sent.
    pub service_update_name: String,
    /// `CacheClusterIds` — "the cache cluster IDs".
    pub cache_cluster_ids: Vec<String>,
    /// `ReplicationGroupIds` — "the replication group IDs".
    pub replication_group_ids: Vec<String>,
}

impl BatchUpdateActionRequest {
    /// Why this request names no resource for `command` to act on, or `None`
    /// when it names at least one.
    ///
    /// Both id lists are modelled `@length(min: 0)`, so the boundary validator
    /// passes a request that names none — and there is then nothing for either
    /// command to act on. The sentence is built here so both handlers refuse
    /// the same condition in the same words and only map it onto their own
    /// modelled error variant.
    pub fn no_resource_refusal(&self, command: UpdateActionCommand) -> Option<String> {
        if !self.cache_cluster_ids.is_empty() || !self.replication_group_ids.is_empty() {
            return None;
        }
        let (verb, preposition) = match command {
            UpdateActionCommand::Apply => ("apply", "to"),
            UpdateActionCommand::Stop => ("stop", "on"),
        };
        Some(format!(
            "CacheClusterIds or ReplicationGroupIds must name at least one resource to {verb} \
             service update {} {preposition}.",
            self.service_update_name
        ))
    }
}

/// The two lists `UpdateActionResultsMessage` reports: the update actions a
/// batch call processed, and the ones it could not.
#[derive(Debug, Clone, Default)]
pub struct UpdateActionResults {
    /// "Update actions that have been processed successfully."
    pub processed: Vec<ProcessedUpdateAction>,
    /// "Update actions that haven't been processed successfully."
    pub unprocessed: Vec<UnprocessedUpdateAction>,
}

/// One update action: the service update being applied, the resource it is
/// being applied to, and how far it has got.
#[derive(Debug, Clone)]
pub struct UpdateActionRecord {
    /// The `ServiceUpdateName` this action applies.
    pub service_update_name: String,
    /// The cache cluster the update is being applied to, when the action was
    /// named through `CacheClusterIds`.
    pub cache_cluster_id: Option<String>,
    /// The replication group the update is being applied to, when the action
    /// was named through `ReplicationGroupIds`.
    pub replication_group_id: Option<String>,
    /// One of `UpdateActionStatus`'s values — see [`UPDATE_ACTION_NOT_APPLIED`].
    pub status: String,
    /// The status change the clock owes this action, if any.
    pub pending_status: Option<PendingTransition>,
    /// When the `BatchApplyUpdateAction` call that started this run asked for
    /// it, which is what `NodeUpdateInitiatedDate` ("the date when the update is
    /// triggered") reports. A stop acts on the run already under way, so it
    /// keeps the instant the apply set.
    pub started_at: SystemTime,
    /// When this action's status last changed — set by the call that moved it
    /// and by [`Self::settle`] to the instant the clock made the change due, so
    /// `UpdateActionStatusModifiedDate` reports when the change HAPPENED rather
    /// than when somebody read it.
    pub status_modified_at: SystemTime,
    /// When the update actually started running on this action's nodes: the
    /// instant it reached [`UPDATE_ACTION_IN_PROGRESS`], which is what
    /// `NodeUpdateStartDate` reports. `None` for a run that was stopped before
    /// it got that far, which never ran on a node.
    pub in_progress_at: Option<SystemTime>,
}

impl UpdateActionRecord {
    /// Materialise every status change the clock owes this action, up to `now`.
    ///
    /// An update action walks a chain of statuses rather than settling once, so
    /// a record nobody read for several spans ends up where the clock has
    /// carried it rather than one step along. This is the ONE write a reporting
    /// operation may make; see [`CacheClusterRecord::settle`].
    pub fn settle(&mut self, now: SystemTime) {
        while let Some(pending) = self.pending_status.clone() {
            if now < pending.due {
                return;
            }
            self.status = pending.status;
            // The change fell due at `pending.due`, so that — and not the
            // instant of the call that noticed it — is when the status changed.
            self.status_modified_at = pending.due;
            if self.status == UPDATE_ACTION_IN_PROGRESS {
                self.in_progress_at = Some(pending.due);
            }
            self.pending_status =
                next_update_action_status(&self.status).map(|next| PendingTransition {
                    due: pending.due + UPDATE_ACTION_TRANSITION,
                    status: next.to_string(),
                });
        }
    }

    /// This record as the modelled `ProcessedUpdateAction`.
    pub fn to_processed(&self) -> ProcessedUpdateAction {
        ProcessedUpdateAction {
            replication_group_id: self.replication_group_id.clone(),
            cache_cluster_id: self.cache_cluster_id.clone(),
            service_update_name: Some(self.service_update_name.clone()),
            update_action_status: Some(self.status.clone()),
        }
    }
}

/// One shard of a replication group an update action runs on: the shard's
/// `NodeGroupId` and its members' `(CacheClusterId, CacheNodeId)`, which is what
/// `NodeGroupMemberUpdateStatus` reports a node by.
#[derive(Debug, Clone)]
pub struct UpdateActionNodeGroup {
    pub node_group_id: String,
    pub members: Vec<(String, String)>,
}

/// The nodes a service update runs on for one resource — which is also WHICH
/// node-level list reports them.
///
/// `UpdateAction` carries two: `CacheNodeUpdateStatus` is "the status of the
/// service update on the cache node", reported by `CacheNodeId` alone, and
/// `NodeGroupUpdateStatus` is "the status of the service update on the node
/// group", whose members name their cluster as well. A cache cluster's update
/// action has the first, a replication group's the second.
#[derive(Debug, Clone)]
pub enum UpdateActionNodes {
    /// One `CacheNodeId` per node of a cache cluster.
    CacheNodes(Vec<String>),
    /// One entry per node group (shard) of a replication group.
    NodeGroups(Vec<UpdateActionNodeGroup>),
}

impl UpdateActionNodes {
    /// How many nodes the update has to run on, which is the denominator of
    /// `UpdateAction$NodesUpdated` — "the progress of the service update on the
    /// replication group".
    fn count(&self) -> usize {
        match self {
            Self::CacheNodes(ids) => ids.len(),
            Self::NodeGroups(groups) => groups.iter().map(|group| group.members.len()).sum(),
        }
    }
}

/// One resource a published service update can have an update action for: the
/// identity the action is reported under, the engine that decides which updates
/// apply to it, when it came into existence and the nodes the update runs on.
#[derive(Debug, Clone)]
pub struct UpdateActionTarget {
    /// Which of `UpdateAction`'s two id members reports this resource.
    pub kind: UpdateActionResource,
    /// The stored id: the key an update action is held under, and the id
    /// reported under `CacheClusterId` / `ReplicationGroupId`.
    pub id: String,
    /// The resource's cache engine, which `ServiceUpdate$Engine` ("the
    /// Elasticache engine to which the update applies") is matched against.
    pub engine: String,
    /// When the resource was created: an update is available to a resource only
    /// once the resource exists, which is what
    /// [`update_action_view`] dates `UpdateActionAvailableDate` from.
    pub created_at: SystemTime,
    pub nodes: UpdateActionNodes,
}

impl UpdateActionTarget {
    /// The key this resource's action for `service_update_name` is held under in
    /// [`ComAmazonawsElasticacheAmazonelasticachev9State::update_actions`].
    pub fn action_key(
        &self,
        service_update_name: &str,
    ) -> (String, UpdateActionResource, String) {
        (
            service_update_name.to_string(),
            self.kind,
            self.id.clone(),
        )
    }
}

/// The node-level view of an update action: the status and the instants every
/// node the action runs on carries.
///
/// This service moves an update action as a whole rather than node by node —
/// nothing in this API addresses one node of an update — so every node of the
/// resource carries the action's own status and instants, and the two per-node
/// lists are this one view repeated for each node the resource holds.
#[derive(Debug, Clone)]
struct NodeUpdateView {
    status: String,
    /// `NodeUpdateStartDate` / `NodeUpdateInitiatedDate`: the instant the update
    /// started running on the node, and the instant it was triggered on it.
    start_date: Option<SystemTime>,
    end_date: Option<SystemTime>,
    initiated_by: Option<String>,
    initiated_date: Option<SystemTime>,
    status_modified_date: SystemTime,
}

impl NodeUpdateView {
    /// The node-level view of `action`, which is `None` for a pair nothing has
    /// applied; `available` is when its action became
    /// [`UPDATE_ACTION_NOT_APPLIED`], which is the only status change such a
    /// pair has had.
    fn of(action: Option<&UpdateActionRecord>, available: SystemTime) -> Self {
        let Some(action) = action else {
            return Self {
                status: UPDATE_ACTION_NOT_APPLIED.to_string(),
                start_date: None,
                end_date: None,
                initiated_by: None,
                initiated_date: None,
                status_modified_date: available,
            };
        };
        Self {
            status: action.status.clone(),
            start_date: action.in_progress_at,
            // "The end date of the update for a node": only a run that has
            // finished has one.
            end_date: update_action_finished(&action.status).then_some(action.status_modified_at),
            initiated_by: Some(NODE_UPDATE_INITIATED_BY_CUSTOMER.to_string()),
            initiated_date: Some(action.started_at),
            status_modified_date: action.status_modified_at,
        }
    }

    /// This view as the modelled `CacheNodeUpdateStatus` of the node
    /// `cache_node_id` names.
    fn to_cache_node(&self, cache_node_id: &str) -> CacheNodeUpdateStatus {
        CacheNodeUpdateStatus {
            cache_node_id: Some(cache_node_id.to_string()),
            node_update_status: Some(self.status.clone()),
            // Applying a service update deletes no node, so there is no
            // deletion date to report.
            node_deletion_date: None,
            node_update_start_date: self.start_date.map(::aws_smithy_types::DateTime::from),
            node_update_end_date: self.end_date.map(::aws_smithy_types::DateTime::from),
            node_update_initiated_by: self.initiated_by.clone(),
            node_update_initiated_date: self.initiated_date.map(::aws_smithy_types::DateTime::from),
            node_update_status_modified_date: Some(::aws_smithy_types::DateTime::from(
                self.status_modified_date,
            )),
        }
    }

    /// This view as the modelled `NodeGroupMemberUpdateStatus` of one node group
    /// member: the same status and instants, reported with the member's cluster.
    fn to_node_group_member(
        &self,
        cache_cluster_id: &str,
        cache_node_id: &str,
    ) -> NodeGroupMemberUpdateStatus {
        let node = self.to_cache_node(cache_node_id);
        NodeGroupMemberUpdateStatus {
            cache_cluster_id: Some(cache_cluster_id.to_string()),
            cache_node_id: node.cache_node_id,
            node_update_status: node.node_update_status,
            node_deletion_date: node.node_deletion_date,
            node_update_start_date: node.node_update_start_date,
            node_update_end_date: node.node_update_end_date,
            node_update_initiated_by: node.node_update_initiated_by,
            node_update_initiated_date: node.node_update_initiated_date,
            node_update_status_modified_date: node.node_update_status_modified_date,
        }
    }
}

/// `UpdateAction$SlaMet` for an action in `status`: whether the update ran on
/// every node by the service update's recommended apply-by date.
///
/// A finished update met the SLA when it finished before that date and missed it
/// otherwise; one that has not finished has missed it only once the date has
/// passed, and until then the question has no answer yet
/// ([`SLA_MET_NOT_APPLICABLE`]).
fn sla_met(
    update: &ServiceUpdateRecord,
    status: &str,
    status_modified: SystemTime,
    now: SystemTime,
) -> &'static str {
    if status == UPDATE_ACTION_COMPLETE {
        if status_modified <= update.recommended_apply_by_date {
            SLA_MET_YES
        } else {
            SLA_MET_NO
        }
    } else if now > update.recommended_apply_by_date {
        SLA_MET_NO
    } else {
        SLA_MET_NOT_APPLICABLE
    }
}

/// The modelled `UpdateAction` for one (service update, resource) pair: what
/// `DescribeUpdateActions` reports about applying `update` to `target`.
///
/// `action` is the record a batch call persisted, or `None` for a pair nothing
/// has started — which IS the `not-applied` status the API reports for it, so
/// the view is computed from the published update and the stored resource rather
/// than requiring a record to exist. Node-level status is reported only when the
/// request asked for it: `ShowNodeLevelUpdateStatus` "dictates whether to
/// include node level update status in the response".
pub fn update_action_view(
    update: &ServiceUpdateRecord,
    target: &UpdateActionTarget,
    action: Option<&UpdateActionRecord>,
    now: SystemTime,
    show_node_level_status: bool,
) -> UpdateAction {
    // "The date that the service update is available to a replication group":
    // the update is available to a resource once BOTH exist, so a resource
    // created after the update was released gains the action when it was
    // created.
    let available = update.release_date.max(target.created_at);
    let node_view = NodeUpdateView::of(action, available);
    let status = node_view.status.clone();
    let total = target.nodes.count();
    // "The progress of the service update on the replication group", read off
    // the node-level statuses: every node of the resource carries the action's
    // own status, so the nodes the update has run on are all of them once it is
    // complete and none of them before.
    let updated = if status == UPDATE_ACTION_COMPLETE { total } else { 0 };
    let (node_groups, cache_nodes) = match (&target.nodes, show_node_level_status) {
        (_, false) => (None, None),
        (UpdateActionNodes::CacheNodes(ids), true) => (
            None,
            Some(
                ids.iter()
                    .map(|id| node_view.to_cache_node(id))
                    .collect::<Vec<CacheNodeUpdateStatus>>(),
            ),
        ),
        (UpdateActionNodes::NodeGroups(groups), true) => (
            Some(
                groups
                    .iter()
                    .map(|group| NodeGroupUpdateStatus {
                        node_group_id: Some(group.node_group_id.clone()),
                        node_group_member_update_status: Some(
                            group
                                .members
                                .iter()
                                .map(|(cluster, node)| {
                                    node_view.to_node_group_member(cluster, node)
                                })
                                .collect(),
                        ),
                    })
                    .collect::<Vec<NodeGroupUpdateStatus>>(),
            ),
            None,
        ),
    };
    UpdateAction {
        replication_group_id: (target.kind == UpdateActionResource::ReplicationGroup)
            .then(|| target.id.clone()),
        cache_cluster_id: (target.kind == UpdateActionResource::CacheCluster)
            .then(|| target.id.clone()),
        service_update_name: Some(update.service_update_name.clone()),
        service_update_release_date: Some(::aws_smithy_types::DateTime::from(update.release_date)),
        service_update_severity: Some(update.severity.clone()),
        service_update_status: Some(update.status(now).to_string()),
        service_update_recommended_apply_by_date: Some(::aws_smithy_types::DateTime::from(
            update.recommended_apply_by_date,
        )),
        service_update_type: Some(update.update_type.clone()),
        update_action_available_date: Some(::aws_smithy_types::DateTime::from(available)),
        update_action_status: Some(status.clone()),
        nodes_updated: Some(format!("{updated}/{total}")),
        update_action_status_modified_date: Some(::aws_smithy_types::DateTime::from(
            node_view.status_modified_date,
        )),
        sla_met: Some(sla_met(update, &status, node_view.status_modified_date, now).to_string()),
        node_group_update_status: node_groups,
        cache_node_update_status: cache_nodes,
        estimated_update_time: Some(update.estimated_update_time.clone()),
        engine: Some(update.engine.clone()),
    }
}

/// How long an access string the store keeps, in characters.
///
/// The model constrains `AccessString` with `@pattern "\S"` alone and
/// `ModifyUser$AppendAccessString` "adds additional user permissions to the
/// access string" a user already has, so the stored rule list grows from call to
/// call and nothing in the model bounds it. This is therefore the emulator's OWN
/// bound on the store, for the reason [`CLUSTER_QUOTA`] gives: it sits far above
/// any rule list an emulator serves, and it is what stops one user's permissions
/// growing without end.
pub const MAX_ACCESS_STRING_LEN: usize = 8192;

/// How the service words the refusal of a rule list it cannot read.
///
/// ONE fixed sentence, whatever is wrong with the list and whichever member
/// carried it: the service states that the access string as a whole is not one,
/// and names neither the member nor the token that is not a rule, so nothing of
/// the request is read back out of the refusal. It is borrowed rather than built,
/// so a caller cannot compose anything into it.
pub const ACCESS_STRING_INVALID: &str = "The access-string is invalid.";

/// Why `access_string` is not an access string, or `None` when it is one.
///
/// The model constrains `AccessString` only with `@pattern "\S"` — any string
/// carrying a non-space character passes the boundary validator — so the rule
/// list itself is parsed here, and a request naming something that is not an ACL
/// rule is refused instead of being stored as a permission set the engine could
/// never apply ([`ACCESS_STRING_INVALID`] is how that refusal reads). The caller
/// maps the refusal onto its own modelled `InvalidParameterValueException`.
///
/// Every way a rule list can fail this test is the SAME condition — the access
/// string is not one this service accepts — so all of them are refused with the
/// one sentence [`ACCESS_STRING_INVALID`] holds, which carries no part of the
/// request: neither the rule that offended nor the member it arrived on is
/// reported, and a caller therefore does not say which member it is checking.
///
/// A SELECTOR — the Redis OSS 7 / Valkey form `(+get ~key1)` — is one rule that
/// carries a rule list of its own, so it is read as a parenthesised group whose
/// inner rules are checked by the same per-rule test. A group that is never
/// closed is refused.
pub fn access_string_rejection(access_string: &str) -> Option<&'static str> {
    // Checked before the list is walked: the length is the one thing about a
    // rule list a caller can grow without end (see [`MAX_ACCESS_STRING_LEN`]),
    // and it bounds the work the walk below does.
    if access_string.chars().count() > MAX_ACCESS_STRING_LEN {
        return Some(ACCESS_STRING_INVALID);
    }
    // Every refusal below is the same one sentence, so the walk only has to
    // decide WHETHER the list is a rule list.
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
                return Some(ACCESS_STRING_INVALID);
            }
            rules += 1;
            continue;
        }
        if token.starts_with('(') {
            if token.ends_with(')') && token.len() > 1 {
                if !is_selector(token) {
                    return Some(ACCESS_STRING_INVALID);
                }
                rules += 1;
            } else {
                selector = Some(token.to_string());
            }
            continue;
        }
        if !is_access_rule(token) {
            return Some(ACCESS_STRING_INVALID);
        }
        rules += 1;
    }
    // A selector whose `)` never arrived, and a string that names no rule at
    // all, are the same condition as a rule the engine does not know: neither is
    // an access string the service can apply.
    if selector.is_some() || rules == 0 {
        return Some(ACCESS_STRING_INVALID);
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

/// The ACL rules that state a rule list's COMMAND BASELINE outright: whether the
/// user starts from every command allowed or every command denied.
///
/// `allcommands` and `nocommands` are the engine's own aliases for `+@all` and
/// `-@all` — they are in [`ACCESS_STRING_KEYWORDS`] for that reason — so a rule
/// list naming any of the four has already said where its command rules start
/// from. Matched case-insensitively, as the engine reads its rule names.
const ACCESS_STRING_COMMAND_BASELINE: [&str; 4] = ["+@all", "-@all", "allcommands", "nocommands"];

/// The baseline term the normalised form carries when the request named none: a
/// new user is created with every command denied, so the expanded rule list
/// starts by denying them all and the rules that follow are what it grants back.
pub const ACCESS_STRING_DENY_ALL: &str = "-@all";

/// True when `rule` states the command baseline — see
/// [`ACCESS_STRING_COMMAND_BASELINE`].
fn is_command_baseline(rule: &str) -> bool {
    ACCESS_STRING_COMMAND_BASELINE
        .iter()
        .any(|baseline| rule.eq_ignore_ascii_case(baseline))
}

/// True when `rule` grants or revokes a command or a command category (`+get`,
/// `-@admin`): the rules whose effect depends on the baseline they start from.
fn is_command_rule(rule: &str) -> bool {
    (rule.starts_with('+') || rule.starts_with('-')) && is_access_rule(rule)
}

/// An access string as the service STORES and REPORTS it: the EXPANDED rule
/// list, which states its command baseline explicitly.
///
/// A Valkey / Redis OSS user is created with no permissions at all — every
/// command denied — and the rules of an access string are applied on top of that
/// starting point, so the expanded form of a rule list that grants commands
/// without saying where it starts from carries the deny-all term
/// ([`ACCESS_STRING_DENY_ALL`]) before its command rules. A list that already
/// states its baseline ([`ACCESS_STRING_COMMAND_BASELINE`]) expands to itself.
/// The term goes in immediately before the FIRST command rule, so the rules it
/// governs follow it, and at the end of a list that names no command rule, whose
/// whole command baseline it then is.
///
/// Nothing else is changed: no other rule is added, removed or reordered, and a
/// selector (`(+get ~key1)`) is one rule of the list it sits in, so the baseline
/// of the list is decided by the rules AROUND it and the selector's own rules are
/// kept exactly as sent. Beyond that only the whitespace between rules is
/// normalised.
pub fn canonical_access_string(access_string: &str) -> String {
    let mut rules: Vec<&str> = access_string.split_whitespace().collect();
    // Where the deny-all term goes, if it is needed at all: before the first
    // command rule of the list ITSELF, so a command rule inside a selector —
    // which governs that selector alone — does not decide it.
    let mut first_command = None;
    let mut inside_selector = false;
    for (index, rule) in rules.iter().enumerate() {
        if inside_selector {
            inside_selector = !rule.ends_with(')');
            continue;
        }
        if rule.starts_with('(') {
            inside_selector = !(rule.ends_with(')') && rule.len() > 1);
            continue;
        }
        if is_command_baseline(rule) {
            // The request said where its command rules start from, so the
            // expanded form is the list as sent.
            return rules.join(" ");
        }
        if first_command.is_none() && is_command_rule(rule) {
            first_command = Some(index);
        }
    }
    rules.insert(first_command.unwrap_or(rules.len()), ACCESS_STRING_DENY_ALL);
    rules.join(" ")
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

/// The modelled `UnprocessedUpdateAction` for one resource a batch call could
/// not act on, carrying the id under the member its kind is reported by.
fn unprocessed_update_action(
    kind: UpdateActionResource,
    id: &str,
    service_update_name: &str,
    error_type: &str,
    error_message: String,
) -> UnprocessedUpdateAction {
    UnprocessedUpdateAction {
        replication_group_id: (kind == UpdateActionResource::ReplicationGroup)
            .then(|| id.to_string()),
        cache_cluster_id: (kind == UpdateActionResource::CacheCluster).then(|| id.to_string()),
        service_update_name: Some(service_update_name.to_string()),
        error_type: Some(error_type.to_string()),
        error_message: Some(error_message),
    }
}

/// The `Authentication` the passwords in `passwords` give a user, or why they
/// are refused.
///
/// Both published rules are enforced here, so every request that carries
/// passwords — `CreateUser`'s `Passwords` and `AuthenticationMode$Passwords`,
/// and `ModifyUser`'s — takes the same ones: at most
/// [`MAX_PASSWORDS_PER_USER`] passwords, each [`PASSWORD_MIN_LENGTH`] to
/// [`PASSWORD_MAX_LENGTH`] characters. A password outside the length rule is
/// refused rather than stored: a user whose password the service would never
/// have accepted must not exist. The passwords themselves are never kept; see
/// [`AuthenticationRecord`].
fn password_authentication(passwords: &[String]) -> Result<AuthenticationRecord, AuthRefusal> {
    if passwords.len() > MAX_PASSWORDS_PER_USER {
        return Err(AuthRefusal::Value(format!(
            "{} passwords were supplied; you can create up to {MAX_PASSWORDS_PER_USER} passwords \
             for each user.",
            passwords.len()
        )));
    }
    for password in passwords {
        if !(PASSWORD_MIN_LENGTH..=PASSWORD_MAX_LENGTH).contains(&password.chars().count()) {
            return Err(AuthRefusal::Value(format!(
                "Passwords length must be between \
                 {PASSWORD_MIN_LENGTH}-{PASSWORD_MAX_LENGTH} characters."
            )));
        }
    }
    // At most `MAX_PASSWORDS_PER_USER`, refused above, so the conversion is
    // exact; the fallback only keeps the handler panic-free.
    let count = i32::try_from(passwords.len()).unwrap_or(i32::MAX);
    Ok(AuthenticationRecord {
        r#type: authentication_type::PASSWORD.to_string(),
        password_count: Some(count),
    })
}

/// Why an `AuthenticationMode` cannot stand beside one of the older
/// authentication members, stated from what the request submitted.
///
/// The two statements of how a user authenticates contradict each other, and the
/// refusal is the only place the caller is told WHICH two it sent: so it names
/// the older directive it carried — the submitted value of the
/// `NoPasswordRequired` flag, or the passwords it supplied — together with the
/// `AuthenticationMode$Type` the same request asked for. `no_password_required`
/// is `None` only when the older directive was `Passwords`; one of the two is
/// always present, since that is what makes this a conflict at all.
fn conflicting_authentication_message(no_password_required: Option<bool>, mode_type: &str) -> String {
    match no_password_required {
        Some(flag) => format!(
            "No password required flag is {flag} but provided authentication type is {mode_type}"
        ),
        None => {
            format!("Passwords are provided but provided authentication type is {mode_type}")
        }
    }
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
        let mode_passwords = mode.passwords.as_deref().unwrap_or(&[]);
        // The authentication type the mode asks for, resolved BEFORE anything
        // else about the mode: every refusal below reports it back, so a request
        // that names no type to report is refused for that first.
        let mode_type = match mode.r#type.as_deref().unwrap_or_default() {
            token @ (input_authentication_type::PASSWORD
            | input_authentication_type::NO_PASSWORD_REQUIRED
            | input_authentication_type::IAM) => token,
            other => {
                return Err(AuthRefusal::Value(format!(
                    "AuthenticationMode.Type must be one of {} — got {other:?}.",
                    // The GENERATED catalog, so a refusal cannot name a
                    // different set from the one the model declares.
                    // `InputAuthenticationType` IS a modelled enum, so the
                    // boundary validator normally refuses an unknown token
                    // before a handler runs; this arm is what keeps an absent
                    // `Type` from being read as one of the three.
                    crate::ops::types::INPUT_AUTHENTICATION_TYPE_VALUES.join(", ")
                )))
            }
        };
        if !passwords.is_empty() || no_password_required.is_some() {
            // Two statements of how the user authenticates: the refusal is built
            // from the pair the request submitted, not from the rule alone.
            return Err(AuthRefusal::Combination(conflicting_authentication_message(
                no_password_required,
                mode_type,
            )));
        }
        return match mode_type {
            input_authentication_type::PASSWORD => {
                if mode_passwords.is_empty() {
                    return Err(AuthRefusal::Combination(format!(
                        "AuthenticationMode.Passwords is required when AuthenticationMode.Type \
                         is {mode_type}."
                    )));
                }
                password_authentication(mode_passwords)
            }
            token => {
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
    /// The engine, lowercased — see [`normalize_access_control_engine`].
    pub engine: String,
    /// The ACL rule list, as [`canonical_access_string`] stores it.
    pub access_string: String,
    pub authentication: AuthenticationRecord,
    /// A `User$Status` token — see [`USER_STATUS_ACTIVE`].
    pub status: String,
    pub arn: String,
    /// The tags the create was given, for `ListTagsForResource` to report and the
    /// two tag mutations to change; see [`CacheClusterRecord::tags`].
    pub tags: Vec<(String, String)>,
    /// When the clock owes this user's removal from the store: the instant
    /// [`USER_DELETION_TRANSITION`] after the `DeleteUser` that moved it to
    /// [`USER_STATUS_DELETING`], and `None` for a user nothing has deleted.
    /// Materialised by [`ComAmazonawsElasticacheAmazonelasticachev9State::settle_users`].
    pub pending_removal: Option<SystemTime>,
    /// The status change the clock owes this user: [`USER_STATUS_ACTIVE`] a span
    /// after the `ModifyUser` that moved it to [`STATUS_MODIFYING`], and `None`
    /// for a user with nothing outstanding. The CHANGE itself is applied at the
    /// call — the `User` shape carries no pending-value member to report it
    /// through — so only the status is owed. Materialised by
    /// [`ComAmazonawsElasticacheAmazonelasticachev9State::settle_users`].
    pub pending_status: Option<PendingTransition>,
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
            // membership is held by the user GROUPS that list the user, so it is
            // derived from those records rather than copied onto the user — see
            // [`ComAmazonawsElasticacheAmazonelasticachev9State::user_group_ids_of`],
            // which is what the handlers pass to [`UserRecord::to_modeled_in`].
            user_group_ids: Some(Vec::new()),
            authentication: Some(self.authentication.to_modeled()),
            arn: Some(self.arn.clone()),
        }
    }

    /// The user as the API reports it, belonging to `user_group_ids`.
    ///
    /// `User$UserGroupIds` is "a list of the user group IDs the user belongs
    /// to", which the user-group records are the one copy of; a handler reads
    /// them with `user_group_ids_of` and hands them here.
    pub fn to_modeled_in(&self, user_group_ids: Vec<String>) -> CreateUserOutput {
        CreateUserOutput {
            user_group_ids: Some(user_group_ids),
            ..self.to_modeled()
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
            // No call creates the built-in user and none deletes it, so the
            // clock never owes its removal.
            pending_removal: None,
            // Nothing has modified it either, so no status change is owed.
            pending_status: None,
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
    /// Every stored user group, keyed by its `UserGroupId` EXACTLY as the
    /// request spelled it: unlike `CacheClusterId` / `UserId` /
    /// `ReplicationGroupId`, the model documents no lowercasing for it, so the
    /// identity a later describe or association resolves is case-sensitive.
    pub user_groups: BTreeMap<String, UserGroupRecord>,
    /// Every stored Global datastore, keyed by its (lowercased)
    /// `GlobalReplicationGroupId`.
    pub global_replication_groups: BTreeMap<String, GlobalReplicationGroupRecord>,
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
    /// `CreateCacheParameterGroup` adds the account's own custom groups beside
    /// them.
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
    /// Every update action a batch call started, keyed by
    /// `(ServiceUpdateName, resource kind, resource key)` — the identity of an
    /// update action in this API is the update together with the resource it is
    /// being applied to, and the KIND is part of that key because a cache
    /// cluster and a replication group may carry the same name while being
    /// different resources. A `BTreeMap` keeps the stable total order a page of
    /// them would index into. A key with no entry is an action nothing has
    /// started, which is the `not-applied` status; see
    /// [`UPDATE_ACTION_NOT_APPLIED`].
    pub update_actions: BTreeMap<(String, UpdateActionResource, String), UpdateActionRecord>,
    /// Every event this service recorded, keyed by [`event_key`] so the map's
    /// order IS the chronological order `DescribeEvents` reports and its
    /// `Marker` indexes into. The log is kept beside the resources rather than
    /// on them, because an event outlives the resource it is about.
    pub events: BTreeMap<String, EventRecord>,
    /// The counter that breaks the tie between two events recorded in the same
    /// millisecond; see [`event_key`].
    pub event_seq: u64,
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
    /// Every stored serverless cache snapshot, keyed by its (lowercased)
    /// `ServerlessCacheSnapshotName` — the resource's identity in this API. A
    /// `BTreeMap`, so `DescribeServerlessCacheSnapshots` has the stable total
    /// order its `NextToken` indexes into. A snapshot outlives the cache it was
    /// taken of, so the records sit beside the caches rather than on them.
    pub serverless_cache_snapshots: BTreeMap<String, ServerlessCacheSnapshotRecord>,
    /// The counter endpoint DNS labels are minted from.
    pub dns_token_seq: u64,
    /// The reserved-cache-node offerings this service publishes, keyed for the
    /// same stable total order. Stocked from [`offering_catalog`] when the store
    /// is created, because the catalog is the service's own and no operation
    /// creates or removes an entry: a describe only reports it, and
    /// `PurchaseReservedCacheNodesOffering` prices a reservation from it.
    pub reserved_cache_nodes_offerings: BTreeMap<String, ReservedCacheNodesOffering>,
    /// The engine default parameters this service publishes, keyed by cache
    /// parameter group family and, inside a family, by
    /// [`ParameterRecord::page_key`] — the total order
    /// `DescribeEngineDefaultParameters`' `Marker` indexes into. Stocked from
    /// [`published_engine_default_parameters`] when the store is created, for the
    /// same reason as the offerings above: the defaults are the engine's, not an
    /// account's, so no operation creates or removes one.
    pub engine_default_parameters: BTreeMap<String, BTreeMap<String, ParameterRecord>>,
    /// Every reservation this account bought, keyed by its
    /// `ReservedCacheNodeId` — "the unique identifier for the reservation",
    /// which is "a customer-specified identifier" and so is held exactly as it
    /// was given. A `BTreeMap` is what gives `DescribeReservedCacheNodes` the
    /// stable total order its `Marker` indexes into.
    pub reserved_cache_nodes: BTreeMap<String, ReservedCacheNodeRecord>,
    /// When each shard this account has tested automatic failover on was tested.
    ///
    /// Not a resource but a RATE: `TestFailover` is bounded at
    /// [`MAX_TEST_FAILOVERS_PER_WINDOW`] shards per [`TEST_FAILOVER_WINDOW`], so
    /// the instants of the accepted calls are what that bound is counted over,
    /// and the ones that have left the window are dropped (see
    /// [`Self::test_failovers_in_window`]).
    pub test_failovers: Vec<SystemTime>,
}

impl Default for ComAmazonawsElasticacheAmazonelasticachev9State {
    /// A fresh store is an account ElastiCache has just become available in: no
    /// customer resources, the default parameter group of every family the
    /// service publishes, the account's built-in `default` user, and the
    /// catalogs this service publishes rather than an account creating them —
    /// the service updates and the reserved-cache-node offerings.
    ///
    /// The engine builds this the first time a handler of this service asks for
    /// its state, so that instant is the anchor the service-update catalog's
    /// release and end dates hang off — fixed for the life of the process, which
    /// is what makes every later read of an update report the same dates.
    ///
    /// The `default` user is seeded here for the same reason as the catalogs:
    /// ElastiCache creates it with the ACCOUNT, before any API call (see
    /// [`DEFAULT_USER_ID`]), so no reporting operation has to write it into the
    /// store on first read — which would put the region of whichever request
    /// read first into its ARN. Its ARN names [`DEFAULT_REGION`], the region
    /// this service treats a request as made in when nothing says otherwise.
    fn default() -> Self {
        Self {
            cache_clusters: BTreeMap::new(),
            cache_subnet_groups: BTreeMap::new(),
            user_groups: BTreeMap::new(),
            serverless_caches: BTreeMap::new(),
            serverless_cache_snapshots: BTreeMap::new(),
            replication_groups: BTreeMap::new(),
            global_replication_groups: BTreeMap::new(),
            cache_parameter_groups: default_parameter_groups(),
            snapshots: BTreeMap::new(),
            service_updates: published_service_updates(SystemTime::now()),
            update_actions: BTreeMap::new(),
            events: BTreeMap::new(),
            event_seq: 0,
            users: built_in_users(),
            cache_engine_versions: published_cache_engine_versions(),
            dns_token_seq: 0,
            reserved_cache_nodes_offerings: offering_catalog(),
            engine_default_parameters: published_engine_default_parameters(),
            reserved_cache_nodes: BTreeMap::new(),
            test_failovers: Vec::new(),
        }
    }
}

impl ComAmazonawsElasticacheAmazonelasticachev9State {
    /// The groups `user_id` belongs to, which is what `User$UserGroupIds`
    /// reports.
    ///
    /// Derived from the user-group records, which are the one copy of a
    /// membership: a group lists its users, so a user's groups are the groups
    /// that list it, in the stable order the map holds them.
    pub fn user_group_ids_of(&self, user_id: &str) -> Vec<String> {
        self.user_groups
            .values()
            .filter(|group| group.user_ids.iter().any(|member| member == user_id))
            .map(|group| group.user_group_id.clone())
            .collect()
    }

    /// The parameter group `name` names, or `None` when this account holds no
    /// such group — the one lookup every handler that resolves a
    /// `CacheParameterGroupName` goes through, so they cannot disagree about
    /// which groups exist. The name "is stored as a lowercase string", so the
    /// lookup is case-insensitive.
    pub fn parameter_group(&self, name: &str) -> Option<&CacheParameterGroupRecord> {
        self.cache_parameter_groups.get(&name.to_ascii_lowercase())
    }

    /// The parameter group `name` names, for a handler that CHANGES it —
    /// resolved exactly the way [`Self::parameter_group`] resolves it, so a
    /// write cannot land on a record a read does not report.
    pub fn parameter_group_mut(&mut self, name: &str) -> Option<&mut CacheParameterGroupRecord> {
        self.cache_parameter_groups
            .get_mut(&name.to_ascii_lowercase())
    }

    /// The engine default parameters of the family `family` names, or `None`
    /// when this service publishes no such family — the one lookup a handler
    /// resolving a `CacheParameterGroupFamily` goes through, so no two of them
    /// can disagree about which families exist.
    pub fn engine_defaults(&self, family: &str) -> Option<&BTreeMap<String, ParameterRecord>> {
        self.engine_default_parameters.get(family)
    }

    /// The engine default parameter of `family` that `name` names, or `None`
    /// when the family publishes no parameter of that name — the one lookup a
    /// handler resolving a `Parameter$ParameterName` against a group's family
    /// goes through, so a parameter a request may set and one a read reports
    /// cannot disagree.
    ///
    /// The catalog is keyed by [`ParameterRecord::page_key`], so the name is
    /// resolved through that key (under either of `EngineDefaults`' two lists)
    /// rather than by scanning the catalog.
    pub fn family_parameter(&self, family: &str, name: &str) -> Option<&ParameterRecord> {
        let parameters = self.engine_defaults(family)?;
        [false, true]
            .into_iter()
            .find_map(|specific| parameters.get(&parameter_page_key(name, specific)))
    }

    /// Record, on every cluster the parameter group `group` is associated with,
    /// that a parameter the engine applies only at a restart has been changed
    /// on it: its nodes keep running the values they were started with until
    /// `RebootCacheCluster` applies the group to them, which is what
    /// `CacheParameterGroupStatus$ParameterApplyStatus` reports (see
    /// [`PARAMETER_APPLY_STATUS_PENDING_REBOOT`]).
    ///
    /// A cluster is associated with the group by name, resolved the way every
    /// other `CacheParameterGroupName` is ([`Self::parameter_group`]), so a
    /// cluster created naming the group in another case is covered too.
    pub fn defer_parameter_change_to_reboot(&mut self, group: &str) {
        for cluster in self.cache_clusters.values_mut() {
            if !cluster.cache_parameter_group_name.eq_ignore_ascii_case(group) {
                continue;
            }
            for node in &mut cluster.nodes {
                node.parameter_group_status = PARAMETER_APPLY_STATUS_PENDING_REBOOT.to_string();
            }
        }
    }

    /// Mint the DNS label a new cluster's endpoints carry: six lowercase
    /// hex digits, the shape of the label in `ConfigurationEndpoint`'s example.
    pub fn mint_dns_token(&mut self) -> String {
        self.dns_token_seq = self.dns_token_seq.wrapping_add(1);
        format!("{:06x}", self.dns_token_seq & 0xff_ffff)
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
            // Nothing owes a just-created cache its removal; only
            // `DeleteServerlessCache` sets this.
            pending_deletion: None,
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
            // A created cache is under no migration: `PrepareMigration` and
            // `StartMigration` are what open one on it.
            migration: None,
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

    /// Store `record` and hand back what was stored, for the reason
    /// [`Self::insert_snapshot`] gives.
    ///
    /// Every route into the map asks
    /// [`Self::serverless_cache_snapshot_refusal`] FIRST, so the store cannot be
    /// grown past its bound by whichever operation mints the snapshot.
    pub fn insert_serverless_cache_snapshot(
        &mut self,
        record: ServerlessCacheSnapshotRecord,
    ) -> &ServerlessCacheSnapshotRecord {
        match self.serverless_cache_snapshots.entry(record.name.clone()) {
            Entry::Vacant(slot) => slot.insert(record),
            Entry::Occupied(mut slot) => {
                slot.insert(record);
                slot.into_mut()
            }
        }
    }

    /// Why no serverless cache snapshot can be stored under `name`, or `None`
    /// when one can be.
    ///
    /// The ONE bound on this map, so every operation that mints a serverless
    /// cache snapshot — `CreateServerlessCacheSnapshot` and the
    /// `FinalSnapshotName` of a `DeleteServerlessCache` — holds to it and
    /// reports it in the same words: a name is unique for the account, and the
    /// store holds at most [`SERVERLESS_CACHE_SNAPSHOT_QUOTA`] snapshots. A
    /// snapshot has no retention limit to expire under, so nothing but this
    /// bound keeps the map finite.
    pub fn serverless_cache_snapshot_refusal(
        &self,
        name: &str,
    ) -> Option<ServerlessSnapshotRefusal> {
        if self.serverless_cache_snapshots.contains_key(name) {
            return Some(ServerlessSnapshotRefusal::AlreadyExists(format!(
                "Serverless cache snapshot {name} already exists."
            )));
        }
        if self.serverless_cache_snapshots.len() >= SERVERLESS_CACHE_SNAPSHOT_QUOTA {
            return Some(ServerlessSnapshotRefusal::QuotaExceeded(format!(
                "This account already holds the maximum of \
                 {SERVERLESS_CACHE_SNAPSHOT_QUOTA} serverless cache snapshots."
            )));
        }
        None
    }

    /// The serverless cache `name` names (the stored, lowercased name), with
    /// every change the clock owes it already materialised: a removal a delete
    /// made due, and then any status change — the serverless counterpart of
    /// [`Self::settled_cluster_mut`], so an operation that branches on a cache's
    /// status reads the status it is owed at this instant, and a cache whose
    /// deletion has completed is reported as gone.
    pub fn settled_serverless_cache_mut(
        &mut self,
        name: &str,
        now: SystemTime,
    ) -> Option<&mut ServerlessCacheRecord> {
        self.purge_deleted_serverless_caches(now);
        // Deliberately two lookups, for the reason
        // [`Self::settled_cluster_mut`] gives: the first ends the record's
        // borrow so the transition's event can be written to the log beside it.
        let settled = {
            let cache = self.serverless_caches.get_mut(name)?;
            cache.settle(now).map(|settled| (cache.name.clone(), settled))
        };
        if let Some((cache_name, settled)) = settled {
            self.record_transition(
                SOURCE_TYPE_SERVERLESS_CACHE,
                &cache_name,
                "Serverless cache",
                &settled,
            );
        }
        self.serverless_caches.get_mut(name)
    }

    /// Drop every serverless cache whose removal the clock already owes — the
    /// caches a `DeleteServerlessCache` put into `DELETING` at least
    /// [`SERVERLESS_CACHE_TRANSITION`] ago.
    ///
    /// Owed by the clock alone (due at this instant whether or not anybody
    /// called), so a reporting operation materialises it too, the same way it
    /// materialises a status change. A user group reports the caches it serves,
    /// so a removed cache leaves that list with it rather than outliving itself
    /// there.
    pub fn purge_deleted_serverless_caches(&mut self, now: SystemTime) {
        let due: Vec<String> = self
            .serverless_caches
            .values()
            .filter(|cache| cache.pending_deletion.is_some_and(|at| now >= at))
            .map(|cache| cache.name.clone())
            .collect();
        for name in due {
            let Some(cache) = self.serverless_caches.remove(&name) else {
                continue;
            };
            if let Some(id) = cache.user_group_id {
                if let Some(group) = self.user_groups.get_mut(&id) {
                    group
                        .serverless_cache_names
                        .retain(|served| served != &name);
                }
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
        // Deliberately two lookups: the first settles the record and ends its
        // borrow so the event the transition records can be written to the log
        // beside it, the second hands the caller the settled record back.
        let settled = {
            let cluster = self.cache_clusters.get_mut(id)?;
            cluster
                .settle(now)
                .map(|settled| (cluster.cache_cluster_id.clone(), settled))
        };
        if let Some((cache_cluster_id, settled)) = settled {
            self.record_transition(
                SOURCE_TYPE_CACHE_CLUSTER,
                &cache_cluster_id,
                "Cache cluster",
                &settled,
            );
        }
        self.cache_clusters.get_mut(id)
    }

    /// The snapshot `name` names (the stored, lowercased name), with any status
    /// change the clock owes it already materialised — the snapshot counterpart
    /// of [`Self::settled_cluster_mut`], so an operation that branches on a
    /// snapshot's status reads the status it is owed at this instant.
    pub fn settled_snapshot_mut(
        &mut self,
        name: &str,
        now: SystemTime,
    ) -> Option<&mut SnapshotRecord> {
        // A snapshot whose removal the clock has already carried through is
        // gone, so it resolves to nothing rather than to a record reporting
        // `deleting` for ever.
        self.purge_deleted_snapshots(now);
        let snapshot = self.snapshots.get_mut(name)?;
        snapshot.settle(now);
        Some(snapshot)
    }

    /// Drop every snapshot whose removal the clock already owes — the snapshots
    /// a `DeleteSnapshot` put into [`STATUS_DELETING`] at least
    /// [`SNAPSHOT_TRANSITION`] ago.
    ///
    /// Owed by the clock alone (due at this instant whether or not anybody
    /// called), so a reporting operation materialises it too, the same way it
    /// materialises a status change — see
    /// [`Self::purge_deleted_serverless_caches`], the same rule for the other
    /// resource a delete only begins.
    pub fn purge_deleted_snapshots(&mut self, now: SystemTime) {
        self.snapshots
            .retain(|_, snapshot| snapshot.pending_removal.is_none_or(|due| now < due));
    }

    /// Materialise every status change the clock owes, for a caller that is
    /// about to report on all of them.
    pub fn settle_all(&mut self, now: SystemTime) {
        // A user's own clock-owed change: the removal `DeleteUser` started.
        self.settle_users(now);
        // A deletion the clock has carried through removes the record before
        // anything is reported from it.
        self.purge_deleted_serverless_caches(now);
        self.purge_deleted_snapshots(now);
        let mut settled: Vec<(String, SettledTransition)> = Vec::new();
        for cluster in self.cache_clusters.values_mut() {
            if let Some(transition) = cluster.settle(now) {
                settled.push((cluster.cache_cluster_id.clone(), transition));
            }
        }
        let mut snapshots: Vec<(String, Option<String>, SettledTransition)> = Vec::new();
        let mut serverless_snapshots: Vec<(String, SettledTransition)> = Vec::new();
        let mut caches: Vec<(String, SettledTransition)> = Vec::new();
        let mut groups: Vec<(String, SettledTransition)> = Vec::new();
        for group in self.replication_groups.values_mut() {
            if let Some(transition) = group.settle(now) {
                groups.push((group.replication_group_id.clone(), transition));
            }
        }
        for cache in self.serverless_caches.values_mut() {
            if let Some(transition) = cache.settle(now) {
                caches.push((cache.name.clone(), transition));
            }
        }
        for snapshot in self.snapshots.values_mut() {
            if let Some(transition) = snapshot.settle(now) {
                snapshots.push((
                    snapshot.snapshot_name.clone(),
                    snapshot.cache_cluster_id.clone(),
                    transition,
                ));
            }
        }
        for snapshot in self.serverless_cache_snapshots.values_mut() {
            if let Some(transition) = snapshot.settle(now) {
                serverless_snapshots.push((snapshot.name.clone(), transition));
            }
        }
        // An update action is not an event source either: `SourceType` has no
        // member for one, so its settled status change records nothing.
        for action in self.update_actions.values_mut() {
            action.settle(now);
        }
        // A reservation is not an event source: `SourceType` has no member for
        // one, so its settled state change records nothing.
        for reservation in self.reserved_cache_nodes.values_mut() {
            reservation.settle(now);
        }
        for (cache_cluster_id, transition) in settled {
            self.record_transition(
                SOURCE_TYPE_CACHE_CLUSTER,
                &cache_cluster_id,
                "Cache cluster",
                &transition,
            );
        }
        for (name, transition) in caches {
            self.record_transition(
                SOURCE_TYPE_SERVERLESS_CACHE,
                &name,
                "Serverless cache",
                &transition,
            );
        }
        for (replication_group_id, transition) in groups {
            self.record_transition(
                SOURCE_TYPE_REPLICATION_GROUP,
                &replication_group_id,
                "Replication group",
                &transition,
            );
        }
        for (snapshot_name, cache_cluster_id, transition) in snapshots {
            // A snapshot is reported under the CLUSTER it was taken from, which
            // is the source the event happened to; see
            // [`SOURCE_TYPE_CACHE_CLUSTER`]. One taken of a replication group
            // has no source cluster, and its own name is the only source left
            // to report it under.
            self.record_transition(
                SOURCE_TYPE_CACHE_CLUSTER,
                cache_cluster_id.as_deref().unwrap_or(&snapshot_name),
                &format!("Snapshot {snapshot_name}"),
                &transition,
            );
        }
        // A serverless cache's snapshot is an event source of its own; see
        // [`SOURCE_TYPE_SERVERLESS_CACHE_SNAPSHOT`].
        for (name, transition) in serverless_snapshots {
            self.record_transition(
                SOURCE_TYPE_SERVERLESS_CACHE_SNAPSHOT,
                &name,
                "Serverless cache snapshot",
                &transition,
            );
        }
        self.expire_events(now);
    }

    /// Materialise the removal of every user whose deletion has fallen due by
    /// `now`.
    ///
    /// `DeleteUser` answers with the user in [`USER_STATUS_DELETING`] — a
    /// non-terminal status — and the clock owes its removal
    /// [`USER_DELETION_TRANSITION`] later, whether or not anybody calls again;
    /// this is where that change is made, which is why a reporting operation may
    /// make it (see [`CacheClusterRecord::settle`]). The event the deletion is
    /// was recorded by the call that asked for it, so nothing is logged here.
    ///
    /// Every path that resolves a user by id materialises it first — the user
    /// operations and `CreateUserGroup`, which builds a group out of users — so
    /// none of them can see a user whose removal has fallen due. A user still
    /// WITHIN the span is a different matter: it exists, in
    /// [`USER_STATUS_DELETING`], and is reported as such.
    pub fn settle_users(&mut self, now: SystemTime) {
        self.users
            .retain(|_, user| user.pending_removal.is_none_or(|due| now < due));
        // The other change a user's clock owes: the `modifying` status a
        // `ModifyUser` left behind settles back to `active` once its span has
        // passed, which is what frees the user to take the next change.
        for user in self.users.values_mut() {
            settle_status(&mut user.status, &mut user.pending_status, now);
        }
    }

    /// Withdraw `user_id` from every user group that lists it.
    ///
    /// `DeleteUser` documents it: "The user will be removed from all user groups
    /// and in turn removed from all replication groups." A group's `user_ids` is
    /// the ONE copy of a membership (see [`Self::user_group_ids_of`]), so this is
    /// where a membership ends — at the call that deletes the user, not when the
    /// record is dropped, so the delete's own answer already reports the user in
    /// no group.
    pub fn withdraw_user_from_groups(&mut self, user_id: &str) {
        for group in self.user_groups.values_mut() {
            group.user_ids.retain(|member| member != user_id);
            // A membership the clock has not applied yet ends here too: a user
            // that has stopped existing must not be carried into a group when
            // the pending change falls due.
            group
                .pending_user_ids_to_add
                .retain(|member| member != user_id);
            group
                .pending_user_ids_to_remove
                .retain(|member| member != user_id);
        }
    }

    /// Append one event to the log, dated at the instant it happened.
    ///
    /// The one place an event is written, so every recorded event carries a key
    /// minted the same way and the log cannot gain two entries under one key.
    pub fn record_event(
        &mut self,
        source_type: &str,
        source_identifier: &str,
        message: impl Into<String>,
        at: SystemTime,
    ) {
        // Monotonic, and saturating rather than wrapping so the tie-break can
        // never run backwards into a key the log already holds. One `u64` is
        // more events than a process can record.
        self.event_seq = self.event_seq.saturating_add(1);
        let key = event_key(at, self.event_seq);
        self.events.insert(
            key,
            EventRecord {
                source_identifier: source_identifier.to_string(),
                source_type: source_type.to_string(),
                message: message.into(),
                at,
            },
        );
        // Retention is owed by the clock, so it is materialised on the write
        // path as well as on the read one: a store that is written to and never
        // described is bounded by [`EVENT_RETENTION`] all the same.
        self.expire_events(at);
    }

    /// Record the event a resource's materialised status change is: `subject`
    /// is what the resource is called in an event's text, and `settled` is the
    /// change the clock made, which the event is dated at.
    ///
    /// The one place a transition is worded, so every resource's transitions
    /// read the same way.
    fn record_transition(
        &mut self,
        source_type: &str,
        source_identifier: &str,
        subject: &str,
        settled: &SettledTransition,
    ) {
        self.record_event(
            source_type,
            source_identifier,
            format!(
                "{subject} status changed from {} to {}",
                settled.from, settled.to
            ),
            settled.due,
        );
    }

    /// Drop every event older than [`EVENT_RETENTION`].
    ///
    /// Expiry is owed by the clock alone — due at this instant whether or not
    /// anybody called — so materialising it is the one write `DescribeEvents`
    /// makes, and it is what bounds the log. [`Self::record_event`] prunes too,
    /// so a store that is only written to is bounded as well.
    pub fn expire_events(&mut self, now: SystemTime) {
        let Some(cutoff) = now.checked_sub(EVENT_RETENTION) else {
            return;
        };
        self.events.retain(|_, event| event.at >= cutoff);
    }

    /// Materialise every change the clock owes a user group: the removal of a
    /// group whose deletion has fallen due, and then the status (with the
    /// membership change) of every group still there.
    ///
    /// `DeleteUserGroup` answers with the group in [`STATUS_DELETING`] — a
    /// non-terminal status — and the clock owes its removal
    /// [`USER_DELETION_TRANSITION`] later, whether or not anybody calls again;
    /// this is where that change is made, which is why a reporting operation may
    /// make it. The event the deletion is was recorded by the call that asked for
    /// it, so nothing is logged here.
    pub fn settle_user_groups(&mut self, now: SystemTime) {
        self.user_groups
            .retain(|_, group| group.pending_removal.is_none_or(|due| now < due));
        for group in self.user_groups.values_mut() {
            group.settle(now);
        }
    }

    /// Materialise every status change the clock owes a replication group.
    pub fn settle_replication_groups(&mut self, now: SystemTime) {
        for group in self.replication_groups.values_mut() {
            group.settle(now);
        }
    }

    /// Materialise every status change the clock owes a Global datastore.
    pub fn settle_global_replication_groups(&mut self, now: SystemTime) {
        for group in self.global_replication_groups.values_mut() {
            group.settle(now);
        }
    }

    /// The replication group `id` names, with any status change the clock owes
    /// it already materialised.
    pub fn settled_replication_group_mut(
        &mut self,
        id: &str,
        now: SystemTime,
    ) -> Option<&mut ReplicationGroupRecord> {
        let group = self.replication_groups.get_mut(id)?;
        group.settle(now);
        Some(group)
    }

    /// Run `edit` over every member CLUSTER of the replication group `group_id`.
    ///
    /// A replication group's settings are the settings of the clusters it is
    /// made of — "the cache parameter group to apply to all of the clusters in
    /// this replication group" — and those clusters are ordinary records in
    /// [`Self::cache_clusters`], which `DescribeCacheClusters` reports. So a
    /// change to the group is carried into them here, through the one walk, and
    /// the two reads cannot disagree about what the account runs.
    pub fn for_each_member_cluster(
        &mut self,
        group_id: &str,
        mut edit: impl FnMut(&mut CacheClusterRecord),
    ) {
        let Some(group) = self.replication_groups.get(group_id) else {
            return;
        };
        for cache_cluster_id in group.member_clusters() {
            if let Some(cluster) = self.cache_clusters.get_mut(&cache_cluster_id) {
                edit(cluster);
            }
        }
    }

    /// Give the shard `node_group_id` of the replication group `group_id`
    /// `replicas` replicas, minting a member CLUSTER for each replica added and
    /// deleting the member cluster of each replica removed.
    ///
    /// A replica of a Valkey / Redis OSS replication group IS a cache cluster —
    /// which is why `insert_replication_group` puts every member in
    /// [`Self::cache_clusters`] — so scaling a shard has to create and remove
    /// those clusters too, or `DescribeCacheClusters` and
    /// `DescribeReplicationGroups` would disagree about what the account runs.
    /// `remove` names the replicas to drop (`ReplicasToRemove`); when it is
    /// empty the last-added replicas go, which is what `ReplicaConfiguration`
    /// alone asks for.
    pub fn set_shard_replica_count(
        &mut self,
        group_id: &str,
        node_group_id: &str,
        replicas: i32,
        remove: &[String],
        now: SystemTime,
    ) {
        let Some(group) = self.replication_groups.get(group_id) else {
            return;
        };
        let Some(shard) = group
            .node_groups
            .iter()
            .find(|shard| shard.node_group_id == node_group_id)
        else {
            return;
        };
        // What the shard has to end up with, decided before anything is
        // written: the primary always stays, the named replicas go first, and
        // then the newest replicas go until the count is right.
        let mut keep: Vec<NodeGroupMemberRecord> = Vec::new();
        let mut drop_clusters: Vec<String> = Vec::new();
        for member in &shard.members {
            if member.current_role == ROLE_PRIMARY || !remove.contains(&member.cache_cluster_id) {
                keep.push(member.clone());
            } else {
                drop_clusters.push(member.cache_cluster_id.clone());
            }
        }
        // How many members the shard ends up with, HELD TO THE PUBLISHED BOUND
        // before anything is allocated: `ReplicasPerNodeGroup` /
        // `NewReplicaCount` is documented "Valid values are 0 to 5"
        // ([`MAX_REPLICAS_PER_NODE_GROUP`]), so a request's count is clamped to
        // that range here as well as refused by the handlers that call this —
        // no request can drive the minting loop below past the bound, whatever
        // a caller forgets to check.
        let replicas = replicas.clamp(0, MAX_REPLICAS_PER_NODE_GROUP);
        let target = usize::try_from(replicas.saturating_add(1)).unwrap_or(1);
        while keep.len() > target {
            if let Some(member) = keep.pop() {
                drop_clusters.push(member.cache_cluster_id);
            }
        }
        // The values a new member cluster takes: the group's own, so a replica
        // runs what the group runs.
        let template = group.clone();
        let mut add: Vec<(String, Option<String>)> = Vec::new();
        let mut next = template
            .node_groups
            .iter()
            .flat_map(|shard| shard.members.iter())
            .filter_map(|member| {
                member
                    .cache_cluster_id
                    .rsplit('-')
                    .next()
                    .and_then(|tail| tail.parse::<u32>().ok())
            })
            .max()
            .unwrap_or(0);
        while keep.len() + add.len() < target {
            next = next.saturating_add(1);
            let zone = availability_zone(&template.region, (keep.len() + add.len()) % 3);
            add.push((
                format!("{}-{next:03}", template.replication_group_id),
                Some(zone),
            ));
        }

        for cluster_id in &drop_clusters {
            self.cache_clusters.remove(cluster_id);
        }
        for (cluster_id, zone) in &add {
            let _ = self.insert_cluster(NewCluster {
                cache_cluster_id: cluster_id.clone(),
                region: template.region.clone(),
                engine: template.engine.clone(),
                engine_version: template.engine_version.clone(),
                cache_node_type: template.cache_node_type.clone(),
                // "For clusters running Valkey or Redis OSS, this value must
                // be 1": a shard's replicas are further member CLUSTERS.
                num_cache_nodes: REDIS_NUM_CACHE_NODES,
                port: template.port,
                availability_zones: zone.clone().into_iter().collect(),
                az_mode: None,
                preferred_maintenance_window: template.preferred_maintenance_window.clone(),
                cache_subnet_group_name: template.cache_subnet_group_name.clone(),
                cache_parameter_group_name: template.cache_parameter_group_name.clone(),
                snapshot_retention_limit: template.snapshot_retention_limit,
                snapshot_window: template.snapshot_window.clone(),
                auto_minor_version_upgrade: template.auto_minor_version_upgrade,
                security_group_ids: Vec::new(),
                notification_topic_arn: None,
                auth_token_enabled: template.auth_token_enabled,
                transit_encryption_enabled: Some(template.transit_encryption_enabled),
                transit_encryption_mode: template.transit_encryption_mode.clone(),
                at_rest_encryption_enabled: Some(template.at_rest_encryption_enabled),
                network_type: template.network_type.clone(),
                ip_discovery: template.ip_discovery.clone(),
                log_delivery_configurations: template.log_delivery_configurations.clone(),
                tags: template.tags.clone(),
                now,
                replication_group_id: Some(template.replication_group_id.clone()),
                replication_group_log_delivery_enabled: Some(
                    !template.log_delivery_configurations.is_empty(),
                ),
            });
        }

        let Some(group) = self.replication_groups.get_mut(group_id) else {
            return;
        };
        let Some(shard) = group
            .node_groups
            .iter_mut()
            .find(|shard| shard.node_group_id == node_group_id)
        else {
            return;
        };
        shard.members = keep;
        for (cluster_id, zone) in add {
            shard.members.push(NodeGroupMemberRecord {
                cache_cluster_id: cluster_id,
                cache_node_id: FIRST_CACHE_NODE_ID.to_string(),
                availability_zone: zone,
                current_role: ROLE_REPLICA.to_string(),
            });
        }
    }

    /// The Global datastore `id` names, with any status change the clock owes it
    /// already materialised.
    pub fn settled_global_replication_group_mut(
        &mut self,
        id: &str,
        now: SystemTime,
    ) -> Option<&mut GlobalReplicationGroupRecord> {
        let group = self.global_replication_groups.get_mut(id)?;
        group.settle(now);
        Some(group)
    }

    /// Build and store a Global datastore, returning what was stored.
    pub fn insert_global_replication_group(
        &mut self,
        new: NewGlobalReplicationGroup,
    ) -> &GlobalReplicationGroupRecord {
        let key = new.global_replication_group_id.to_ascii_lowercase();
        let record = GlobalReplicationGroupRecord::new(new);
        match self.global_replication_groups.entry(key) {
            Entry::Vacant(slot) => slot.insert(record),
            Entry::Occupied(mut slot) => {
                slot.insert(record);
                slot.into_mut()
            }
        }
    }

    /// The resource a batch update-action call named, resolved against this
    /// store: the key it is held under and the cache engine it runs, or the
    /// `ErrorType` and message that say this account holds no such resource.
    fn resolve_update_action_target(
        &self,
        kind: UpdateActionResource,
        id: &str,
    ) -> Result<(String, String), (&'static str, String)> {
        match kind {
            UpdateActionResource::CacheCluster => {
                // `CacheClusterId` "is stored as a lowercase string", so the
                // lookup lowercases too.
                let key = id.to_ascii_lowercase();
                match self.cache_clusters.get(&key) {
                    Some(cluster) => Ok((key, cluster.engine.clone())),
                    None => Err((
                        CACHE_CLUSTER_NOT_FOUND_ERROR_TYPE,
                        cache_cluster_not_found_message(id),
                    )),
                }
            }
            UpdateActionResource::ReplicationGroup => {
                // A `ReplicationGroupId` "is stored as a lowercase string" like
                // every other identifier in this API, so the lookup lowercases
                // too, and the group's own `Engine` is what decides whether the
                // update applies to it.
                let key = id.to_ascii_lowercase();
                match self.replication_groups.get(&key) {
                    Some(group) => Ok((key, group.engine.clone())),
                    None => Err((
                        REPLICATION_GROUP_NOT_FOUND_ERROR_TYPE,
                        replication_group_not_found_message(id),
                    )),
                }
            }
        }
    }

    /// Run `command` for the service update `request` names over each resource
    /// it names, in request order, and report what came of each one — or `None`
    /// when this account's published catalogue holds no such update, which is
    /// the one condition these operations model a fault for.
    ///
    /// The accounting is per resource: a resource this account holds, that the
    /// update applies to, and whose action the command can move, is reported
    /// under `ProcessedUpdateActions` with the status it is now in; every other
    /// named resource is reported under `UnprocessedUpdateActions` with the
    /// reason, and neither refuses the whole call.
    pub fn run_update_actions(
        &mut self,
        command: UpdateActionCommand,
        request: &BatchUpdateActionRequest,
        now: SystemTime,
    ) -> Option<UpdateActionResults> {
        let mut results = UpdateActionResults::default();
        // `ServiceUpdate$Engine` is "the Elasticache engine to which the update
        // applies", so an update released for one engine is no action on a
        // resource running another. Resolving the name here is also what makes
        // the not-found answer unmissable: there is no results message for an
        // update this account does not hold.
        let update_engine = self
            .service_updates
            .get(&request.service_update_name)
            .map(|update| update.engine.clone())?;
        let named: Vec<(UpdateActionResource, &String)> = request
            .cache_cluster_ids
            .iter()
            .map(|id| (UpdateActionResource::CacheCluster, id))
            .chain(
                request
                    .replication_group_ids
                    .iter()
                    .map(|id| (UpdateActionResource::ReplicationGroup, id)),
            )
            .collect();
        for (kind, id) in named {
            let name = request.service_update_name.clone();
            let (key, engine) = match self.resolve_update_action_target(kind, id) {
                Ok(target) => target,
                Err((error_type, message)) => {
                    results.unprocessed.push(unprocessed_update_action(
                        kind,
                        id,
                        &name,
                        error_type,
                        message,
                    ));
                    continue;
                }
            };
            if engine != update_engine {
                let message = format!(
                    "Service update {name} applies to {update_engine}, not to {} {id} running \
                     {engine}.",
                    kind.label()
                );
                results.unprocessed.push(unprocessed_update_action(
                    kind,
                    id,
                    &name,
                    UPDATE_ACTION_INVALID_ERROR_TYPE,
                    message,
                ));
                continue;
            }
            let action_key = (name.clone(), kind, key.clone());
            // The status the command acts on is the one the clock has carried
            // the stored action to; a pair with no record has never been
            // applied.
            let (status, previous) = match self.update_actions.get_mut(&action_key) {
                Some(action) => {
                    action.settle(now);
                    (action.status.clone(), Some(action.clone()))
                }
                None => (UPDATE_ACTION_NOT_APPLIED.to_string(), None),
            };
            let next = match command {
                UpdateActionCommand::Apply if update_action_in_flight(&status) => {
                    let message = format!(
                        "Update action for service update {name} on {} {id} is {status}, so it \
                         is already under way.",
                        kind.label()
                    );
                    results.unprocessed.push(unprocessed_update_action(
                        kind,
                        id,
                        &name,
                        UPDATE_ACTION_INVALID_ERROR_TYPE,
                        message,
                    ));
                    continue;
                }
                UpdateActionCommand::Apply => UPDATE_ACTION_WAITING_TO_START,
                UpdateActionCommand::Stop if !update_action_stoppable(&status) => {
                    let message = format!(
                        "Update action for service update {name} on {} {id} is {status}, so \
                         there is nothing to stop.",
                        kind.label()
                    );
                    results.unprocessed.push(unprocessed_update_action(
                        kind,
                        id,
                        &name,
                        UPDATE_ACTION_INVALID_ERROR_TYPE,
                        message,
                    ));
                    continue;
                }
                UpdateActionCommand::Stop => UPDATE_ACTION_STOPPING,
            };
            let record = UpdateActionRecord {
                service_update_name: name,
                cache_cluster_id: (kind == UpdateActionResource::CacheCluster)
                    .then(|| key.clone()),
                replication_group_id: (kind == UpdateActionResource::ReplicationGroup)
                    .then(|| key.clone()),
                status: next.to_string(),
                pending_status: next_update_action_status(next).map(|after| PendingTransition {
                    due: now + UPDATE_ACTION_TRANSITION,
                    status: after.to_string(),
                }),
                // An apply STARTS a run, so the run's instants are this call's;
                // a stop acts on the run already under way and keeps them, so
                // the node-level dates it reports still say when that run was
                // triggered and when it reached the nodes.
                started_at: match command {
                    UpdateActionCommand::Apply => now,
                    UpdateActionCommand::Stop => {
                        previous.as_ref().map_or(now, |earlier| earlier.started_at)
                    }
                },
                in_progress_at: match command {
                    UpdateActionCommand::Apply => None,
                    UpdateActionCommand::Stop => {
                        previous.as_ref().and_then(|earlier| earlier.in_progress_at)
                    }
                },
                status_modified_at: now,
            };
            // The entry is read back out of the map, so the response reports
            // the action this service persisted rather than a copy beside it.
            let stored = match self.update_actions.entry(action_key) {
                Entry::Vacant(slot) => slot.insert(record),
                Entry::Occupied(mut slot) => {
                    slot.insert(record);
                    slot.into_mut()
                }
            };
            results.processed.push(stored.to_processed());
        }
        Some(results)
    }

    /// Every resource this account holds that a published service update can
    /// have an update action for, in a stable order (the cache clusters by id,
    /// then the replication groups by id).
    ///
    /// An update action is NOT a resource a caller creates: ElastiCache has one
    /// for each (published service update, resource) pair the update applies to,
    /// which is why `DescribeUpdateActions` reports a pair nothing has started
    /// as [`UPDATE_ACTION_NOT_APPLIED`]. The pairs are built from this list and
    /// the published catalog on every read rather than stored, so a resource
    /// created or deleted between two describes changes the answer exactly as it
    /// does in the API — and the stored [`UpdateActionRecord`]s are only the
    /// pairs a batch call has moved off that status.
    ///
    /// A member cluster of a replication group is an ordinary cache cluster in
    /// this store's one map, so it appears here in its own right: the batch
    /// operations accept its id and persist an action under it, and a report
    /// that left it out would hide that action.
    pub fn update_action_targets(&self) -> Vec<UpdateActionTarget> {
        let clusters = self.cache_clusters.values().map(|cluster| UpdateActionTarget {
            kind: UpdateActionResource::CacheCluster,
            id: cluster.cache_cluster_id.clone(),
            engine: cluster.engine.clone(),
            created_at: cluster.created_at,
            nodes: UpdateActionNodes::CacheNodes(
                cluster
                    .nodes
                    .iter()
                    .map(|node| node.cache_node_id.clone())
                    .collect(),
            ),
        });
        let groups = self
            .replication_groups
            .values()
            .map(|group| UpdateActionTarget {
                kind: UpdateActionResource::ReplicationGroup,
                id: group.replication_group_id.clone(),
                engine: group.engine.clone(),
                created_at: group.created_at,
                nodes: UpdateActionNodes::NodeGroups(
                    group
                        .node_groups
                        .iter()
                        .map(|shard| UpdateActionNodeGroup {
                            node_group_id: shard.node_group_id.clone(),
                            members: shard
                                .members
                                .iter()
                                .map(|member| {
                                    (
                                        member.cache_cluster_id.clone(),
                                        member.cache_node_id.clone(),
                                    )
                                })
                                .collect(),
                        })
                        .collect(),
                ),
            });
        clusters.chain(groups).collect()
    }

    /// How many CUSTOM cache parameter groups this account holds — the count
    /// [`PARAMETER_GROUP_QUOTA`] is taken against. The family defaults the store
    /// is seeded with are the service's own, so they do not count against an
    /// account's quota.
    pub fn custom_parameter_group_count(&self) -> usize {
        self.cache_parameter_groups
            .keys()
            .filter(|name| !name.starts_with(DEFAULT_PARAMETER_GROUP_PREFIX))
            .count()
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
            // A created group has nothing deferred, no daily snapshot source
            // and no auth-token update behind it; `ModifyReplicationGroup` is
            // what writes all three.
            pending_modified_values: ReplicationGroupPendingValues::default(),
            snapshotting_cluster_id: None,
            auth_token_last_strategy: None,
            // A created group is under no migration: `StartMigration` is what
            // opens one on it.
            migration: None,
            // A created group belongs to no Global datastore: the Global
            // datastore operations are what make a group a member, and
            // `CreateReplicationGroup` does not create the secondary of one.
            global_replication_group_id: None,
            global_member_role: None,
        };
        self.replication_groups
            .insert(new.replication_group_id.clone(), record);
    }

    /// How many shards this account has tested automatic failover on inside the
    /// rolling [`TEST_FAILOVER_WINDOW`] that ends at `now` — the count
    /// [`MAX_TEST_FAILOVERS_PER_WINDOW`] bounds.
    ///
    /// Counting drops the calls that have left the window, which is a write the
    /// CLOCK owes: a call leaves a rolling window at a fixed instant whether or
    /// not anybody calls again.
    pub fn test_failovers_in_window(&mut self, now: SystemTime) -> usize {
        if let Some(cutoff) = now.checked_sub(TEST_FAILOVER_WINDOW) {
            self.test_failovers.retain(|at| *at >= cutoff);
        }
        self.test_failovers.len()
    }

    /// Record that automatic failover was tested on one shard at `at`; see
    /// [`Self::test_failovers_in_window`].
    pub fn record_test_failover(&mut self, at: SystemTime) {
        self.test_failovers.push(at);
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

    /// The tags the resource `resource_arn` names carries, or why that ARN names
    /// no resource of this service.
    ///
    /// The ONE resolution `AddTagsToResource`, `ListTagsForResource` and
    /// `RemoveTagsFromResource` share, so they cannot disagree about which
    /// resource an ARN is about, nor report different tags for one resource:
    /// tags live on the resource's own record (`CacheClusterRecord::tags` and
    /// its siblings), which is also what a create's `Tags` member writes.
    ///
    /// A resource is matched on the ARN this service MINTED for it and stored on
    /// its record, so an ARN naming an existing name in another Region, account
    /// or partition resolves to nothing rather than to this store's copy of that
    /// name. A cache parameter group is the one kind whose record carries no
    /// Region of its own — a group's ARN is built from the Region of the request
    /// reporting it ([`parameter_group_arn`]) — so for that kind the ARN is
    /// matched against the one built from its own Region field.
    ///
    /// The kind this emulator mints no resource of yet (the EC2-Classic cache
    /// security group) can match nothing, which is exactly what its not-found
    /// fault says.
    pub fn resource_tags(
        &self,
        resource_arn: &str,
    ) -> Result<&Vec<(String, String)>, TagTargetRejection> {
        let parts = tag_target_parts(resource_arn)?;
        let found = match parts.kind {
            TaggedResourceKind::CacheCluster => self
                .cache_clusters
                .values()
                .find(|record| record.arn == resource_arn)
                .map(|record| &record.tags),
            TaggedResourceKind::CacheParameterGroup => self
                .cache_parameter_groups
                .values()
                .find(|record| parameter_group_arn(parts.region, &record.name) == resource_arn)
                .map(|record| &record.tags),
            TaggedResourceKind::CacheSubnetGroup => self
                .cache_subnet_groups
                .values()
                .find(|record| record.arn == resource_arn)
                .map(|record| &record.tags),
            TaggedResourceKind::GlobalReplicationGroup => self
                .global_replication_groups
                .values()
                .find(|record| record.arn == resource_arn)
                .map(|record| &record.tags),
            TaggedResourceKind::ReplicationGroup => self
                .replication_groups
                .values()
                .find(|record| record.arn == resource_arn)
                .map(|record| &record.tags),
            TaggedResourceKind::ReservedCacheNode => self
                .reserved_cache_nodes
                .values()
                .find(|record| record.arn == resource_arn)
                .map(|record| &record.tags),
            TaggedResourceKind::ServerlessCache => self
                .serverless_caches
                .values()
                .find(|record| record.arn == resource_arn)
                .map(|record| &record.tags),
            TaggedResourceKind::ServerlessCacheSnapshot => self
                .serverless_cache_snapshots
                .values()
                .find(|record| record.arn == resource_arn)
                .map(|record| &record.tags),
            TaggedResourceKind::Snapshot => self
                .snapshots
                .values()
                .find(|record| record.arn == resource_arn)
                .map(|record| &record.tags),
            TaggedResourceKind::User => self
                .users
                .values()
                .find(|record| record.arn == resource_arn)
                .map(|record| &record.tags),
            TaggedResourceKind::UserGroup => self
                .user_groups
                .values()
                .find(|record| record.arn == resource_arn)
                .map(|record| &record.tags),
            TaggedResourceKind::CacheSecurityGroup => None,
        };
        found.ok_or_else(|| parts.not_found())
    }

    /// [`Self::resource_tags`] with the write capability, for the two tagging
    /// operations that CHANGE a resource's tags. The resolution is the same one,
    /// arm for arm: a caller that can read a resource's tags through an ARN is
    /// the caller that can write them through it.
    pub fn resource_tags_mut(
        &mut self,
        resource_arn: &str,
    ) -> Result<&mut Vec<(String, String)>, TagTargetRejection> {
        let parts = tag_target_parts(resource_arn)?;
        let found = match parts.kind {
            TaggedResourceKind::CacheCluster => self
                .cache_clusters
                .values_mut()
                .find(|record| record.arn == resource_arn)
                .map(|record| &mut record.tags),
            TaggedResourceKind::CacheParameterGroup => self
                .cache_parameter_groups
                .values_mut()
                .find(|record| parameter_group_arn(parts.region, &record.name) == resource_arn)
                .map(|record| &mut record.tags),
            TaggedResourceKind::CacheSubnetGroup => self
                .cache_subnet_groups
                .values_mut()
                .find(|record| record.arn == resource_arn)
                .map(|record| &mut record.tags),
            TaggedResourceKind::GlobalReplicationGroup => self
                .global_replication_groups
                .values_mut()
                .find(|record| record.arn == resource_arn)
                .map(|record| &mut record.tags),
            TaggedResourceKind::ReplicationGroup => self
                .replication_groups
                .values_mut()
                .find(|record| record.arn == resource_arn)
                .map(|record| &mut record.tags),
            TaggedResourceKind::ReservedCacheNode => self
                .reserved_cache_nodes
                .values_mut()
                .find(|record| record.arn == resource_arn)
                .map(|record| &mut record.tags),
            TaggedResourceKind::ServerlessCache => self
                .serverless_caches
                .values_mut()
                .find(|record| record.arn == resource_arn)
                .map(|record| &mut record.tags),
            TaggedResourceKind::ServerlessCacheSnapshot => self
                .serverless_cache_snapshots
                .values_mut()
                .find(|record| record.arn == resource_arn)
                .map(|record| &mut record.tags),
            TaggedResourceKind::Snapshot => self
                .snapshots
                .values_mut()
                .find(|record| record.arn == resource_arn)
                .map(|record| &mut record.tags),
            TaggedResourceKind::User => self
                .users
                .values_mut()
                .find(|record| record.arn == resource_arn)
                .map(|record| &mut record.tags),
            TaggedResourceKind::UserGroup => self
                .user_groups
                .values_mut()
                .find(|record| record.arn == resource_arn)
                .map(|record| &mut record.tags),
            TaggedResourceKind::CacheSecurityGroup => None,
        };
        found.ok_or_else(|| parts.not_found())
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

/// Why every operation on the legacy cache-security-group surface is refused.
///
/// A cache security group is the pre-VPC (EC2-Classic) access-control mechanism:
/// `CreateCacheSecurityGroup` documents that "cache security groups are only used
/// when you are creating a cluster outside of an Amazon Virtual Private Cloud
/// (Amazon VPC). If you are creating a cluster inside of a VPC, use a cache
/// subnet group instead." This emulator provisions no EC2-Classic network — a
/// cluster is always placed in a VPC subnet group ([`CacheSubnetGroupRecord`]) —
/// so the account it presents is VPC-only, which after EC2-Classic retirement is
/// the only kind of account AWS has. Every operation on the surface is therefore
/// refused account-wide with the modelled `InvalidParameterValueException`
/// (awsQuery code `InvalidParameterValue`, HTTP 400) rather than a per-name
/// not-found: the construct is unavailable, not the name. The status and the code
/// are the contract; this wording is the one AWS was recorded answering with.
pub const CACHE_SECURITY_GROUPS_UNSUPPORTED: &str =
    "Use of cache security groups is not permitted in this API version for your account.";

/// The length a user's password must have.
///
/// ElastiCache enforces the same password rule for a user as `AuthToken`
/// documents for a cluster: "Must be at least 16 characters and no more than 128
/// characters in length." A password outside it is refused with
/// `InvalidParameterValueException`, and no user is created — see
/// [`password_authentication`], which is where every password-bearing request
/// goes through it.
pub const PASSWORD_MIN_LENGTH: usize = 16;
/// See [`PASSWORD_MIN_LENGTH`].
pub const PASSWORD_MAX_LENGTH: usize = 128;

/// How many user groups the store holds before a create is refused with the
/// modelled quota fault. The emulator's own bound on the store, for the reason
/// [`CLUSTER_QUOTA`] gives, and the counterpart of [`USER_QUOTA`].
pub const USER_GROUP_QUOTA: usize = 100;

/// A replication group's and a Global datastore's statuses, from
/// `ReplicationGroup$Status` ("creating, available, modifying, deleting,
/// create-failed, snapshotting") and `GlobalReplicationGroup$Status`.
pub const STATUS_PRIMARY_ONLY: &str = "primary-only";

/// The `AutomaticFailoverStatus` / `MultiAZStatus` values, from those enums.
pub const FAILOVER_ENABLED: &str = "enabled";
/// See [`FAILOVER_ENABLED`].
pub const FAILOVER_DISABLED: &str = "disabled";

/// The `ClusterMode` values, from that enum (`enabled`, `disabled`,
/// `compatible`).
pub const CLUSTER_MODE_DISABLED: &str = "disabled";
/// See [`CLUSTER_MODE_DISABLED`].
pub const CLUSTER_MODE_ENABLED: &str = "enabled";

/// The role a member of a Global datastore has, from
/// `GlobalReplicationGroupMember$Role`: "Indicates the role of the replication
/// group, primary or secondary."
pub const GLOBAL_ROLE_PRIMARY: &str = "primary";
/// See [`GLOBAL_ROLE_PRIMARY`].
pub const GLOBAL_ROLE_SECONDARY: &str = "secondary";

/// Materialise a status change the clock owes a resource, if it is due.
///
/// The ONE write a reporting operation may make: the transition was owed at this
/// instant whether or not anybody called. Shared by every record that carries a
/// [`PendingTransition`] so the rule has one implementation.
fn settle_status(
    status: &mut String,
    pending: &mut Option<PendingTransition>,
    now: SystemTime,
) -> bool {
    let Some(transition) = pending.as_ref() else {
        return false;
    };
    if now < transition.due {
        return false;
    }
    *status = transition.status.clone();
    *pending = None;
    true
}

/// The transition the clock owes a resource that a call has just put in a
/// transient status: `status` once [`CLUSTER_TRANSITION`] has passed.
fn transition_to(status: &str, now: SystemTime) -> Option<PendingTransition> {
    Some(PendingTransition {
        due: now + CLUSTER_TRANSITION,
        status: status.to_string(),
    })
}

/// The transition the clock owes a just-created or just-modified user group:
/// [`USER_STATUS_ACTIVE`], which is what `UserGroup$Status` settles at.
pub fn user_group_transition(now: SystemTime) -> Option<PendingTransition> {
    transition_to(USER_STATUS_ACTIVE, now)
}

/// The transition the clock owes a just-modified user: [`USER_STATUS_ACTIVE`],
/// which is what `User$Status` ("Can be \"active\", \"modifying\" or
/// \"deleting\"") settles at once the change has been carried through.
pub fn user_transition(now: SystemTime) -> Option<PendingTransition> {
    transition_to(USER_STATUS_ACTIVE, now)
}

/// Why the user cannot take a change at this instant, or `None` when it can.
///
/// `InvalidUserStateFault` is "The user is not in active state", so a user still
/// carrying a change the clock has not settled, or one on its way out, takes no
/// further change. Stated once, over the settled status, so every operation that
/// changes a user agrees about which statuses it accepts.
pub fn user_state_refusal(user: &UserRecord) -> Option<String> {
    (user.status != USER_STATUS_ACTIVE).then(|| {
        format!(
            "User {} is in {} state, not active.",
            user.user_id, user.status
        )
    })
}

/// Why a user group cannot take a change at this instant, or `None` when it
/// can.
///
/// `InvalidUserGroupStateFault` is "the user group is not in an active state",
/// so a group that is still `creating`, already `modifying` or on its way out
/// is refused. Stated once, over the settled status, so every operation that
/// changes a group agrees about which statuses it accepts.
pub fn user_group_state_refusal(group: &UserGroupRecord) -> Option<String> {
    (group.status != USER_STATUS_ACTIVE).then(|| {
        format!(
            "User group {} is in status {}; it must be {} to be changed.",
            group.user_group_id, group.status, USER_STATUS_ACTIVE
        )
    })
}

/// `UserGroup$Status` of a group whose creation did not complete. Nothing in
/// this emulator puts a group in it — a create either is refused or settles
/// `active` — but it is one of the two statuses a DELETE admits, so the delete
/// guard below names it rather than assuming `active` is the only one.
pub const USER_GROUP_STATUS_CREATE_FAILED: &str = "create-failed";

/// Why the user group cannot be DELETED at this instant, or `None` when it can.
///
/// A delete is not a change to the group: `InvalidUserGroupStateFault` ("the
/// user group is not in an active state") refuses it while the group is still
/// being created, is carrying a change the clock has not settled, or is already
/// on its way out — but a group whose creation FAILED can be deleted, so the
/// refusal names BOTH admissible statuses rather than only `active`. The
/// group's own id and its current status are the variable parts; the tail
/// states the retry condition, which is what a caller waiting to delete needs.
pub fn user_group_delete_state_refusal(group: &UserGroupRecord) -> Option<String> {
    let deletable = [USER_STATUS_ACTIVE, USER_GROUP_STATUS_CREATE_FAILED];
    (!deletable.contains(&group.status.as_str())).then(|| {
        format!(
            "User group {} has status {}. Only {} or {} user groups can be deleted.",
            group.user_group_id,
            group.status,
            USER_STATUS_ACTIVE,
            USER_GROUP_STATUS_CREATE_FAILED
        )
    })
}

/// True when `value` is one of the engines access control exists for, lowercased
/// by the caller; see [`USER_ENGINES`].
pub fn is_user_engine(value: &str) -> bool {
    USER_ENGINES.contains(&value)
}

/// The ARN of a user group.
pub fn user_group_arn(region: &str, user_group_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("usergroup:{user_group_id}"))
}

/// The ARN of a Global datastore.
pub fn global_replication_group_arn(region: &str, global_replication_group_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("globalreplicationgroup:{global_replication_group_id}"),
    )
}

/// How many characters the prefix a Global datastore id carries has; see
/// [`global_datastore_prefix`].
pub const GLOBAL_ID_PREFIX_LEN: usize = 5;

/// The Global datastore id prefixes this service's own sources name: the one the
/// model publishes — "a Global datastore ID created in the US-West-1 region will
/// begin with `dsdfu`"
/// (`CreateGlobalReplicationGroupMessage$GlobalReplicationGroupIdSuffix`) — and
/// the one every Global datastore id AWS was recorded answering with in
/// `us-east-1` carries. Every other region's prefix is minted by
/// [`global_datastore_prefix`].
const PUBLISHED_GLOBAL_ID_PREFIXES: [(&str, &str); 2] =
    [("us-east-1", "ldgnf"), ("us-west-1", "dsdfu")];

/// The prefix a Global datastore id created in `region` begins with.
///
/// "Amazon ElastiCache automatically applies a prefix to the Global datastore ID
/// when it is created. Each Amazon Region has its own prefix … The suffix,
/// combined with the auto-generated prefix, guarantees uniqueness of the Global
/// datastore name across multiple regions." The model publishes the prefix of
/// one region ([`PUBLISHED_GLOBAL_ID_PREFIXES`]); for every other region the
/// prefix is MINTED here from the region name, so it has the published shape
/// ([`GLOBAL_ID_PREFIX_LEN`] lowercase letters), differs from region to region,
/// and is the same every time the same region is asked — which is what makes a
/// stored id stable across reads.
pub fn global_datastore_prefix(region: &str) -> String {
    let region = region.to_ascii_lowercase();
    if let Some((_, prefix)) = PUBLISHED_GLOBAL_ID_PREFIXES
        .iter()
        .find(|(published, _)| *published == region)
    {
        return (*prefix).to_string();
    }
    // FNV-1a over the region name: a stable, well-spread function of the whole
    // name, so two regions that differ anywhere get different prefixes.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in region.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (0..GLOBAL_ID_PREFIX_LEN)
        .map(|index| {
            let letter = (hash >> (index * 8)) % 26;
            char::from(b'a' + u8::try_from(letter).unwrap_or(0))
        })
        .collect()
}

/// The id a Global datastore created in `region` with `suffix` is stored and
/// reported under: the region's [`global_datastore_prefix`] and the suffix the
/// request named, lowercased like every other identifier of this service.
pub fn global_replication_group_id(region: &str, suffix: &str) -> String {
    format!(
        "{}-{}",
        global_datastore_prefix(region),
        suffix.to_ascii_lowercase()
    )
}

/// The keyspace each of `count` node groups owns, as `NodeGroup$Slots` and
/// `GlobalNodeGroup$Slots` report it.
///
/// One rule, one implementation: each range is [`default_slots`], the split a
/// replication group's shards already take, so a Global datastore's slice
/// groups and a replication group's shards cannot divide the keyspace
/// differently.
pub fn even_slot_ranges(count: usize) -> Vec<String> {
    let shards = i32::try_from(count).unwrap_or(i32::MAX);
    (0..shards)
        .map(|index| default_slots(shards, index))
        .collect()
}

/// The statuses a Global datastore accepts a change in: settled ones. A
/// datastore still `creating`, `modifying` or on its way out is refused with
/// `InvalidGlobalReplicationGroupStateFault`.
pub const GLOBAL_ACTIONABLE_STATUSES: [&str; 2] = [STATUS_AVAILABLE, STATUS_PRIMARY_ONLY];

/// Why this service refuses a deferred change.
///
/// `ApplyImmediately` chooses between applying a change now and applying it "during
/// the next maintenance window". The `GlobalReplicationGroup` shape models NO
/// pending-values member, so a deferred change would have nowhere to be
/// reported: a caller would get a 200 and then read back the old settings with
/// nothing saying a change is coming. It is refused with the modelled
/// `InvalidParameterValueException` instead of being accepted and lost.
pub const APPLY_IMMEDIATELY_REQUIRED: &str =
    "Invalid value false for ApplyImmediately: this emulator applies a Global datastore change \
     at once, and the GlobalReplicationGroup shape has no member to report a pending one.";

/// Why a replica-count request is refused, and with which modelled error.
///
/// `IncreaseReplicaCount` and `DecreaseReplicaCount` declare the same two
/// client-input errors, so the shared argument check names which one its answer
/// is and each handler lifts it into its own error enum.
#[derive(Debug, Clone)]
pub enum ReplicaCountRefusal {
    /// `InvalidParameterValueException`: one value is not one the service
    /// serves.
    Value(String),
    /// `InvalidParameterCombinationException`: two members cannot hold
    /// together.
    Combination(String),
}

/// How many replicas a replica-count request asks each shard to end up with.
#[derive(Debug, Clone)]
pub enum ReplicaTargets {
    /// `NewReplicaCount`: "the number of read replica nodes you want at the end
    /// of this operation" — the same count for every shard of the group.
    EveryShard(i32),
    /// `ReplicaConfiguration`: a count per named shard.
    PerShard(Vec<(String, i32)>),
}

/// The per-shard replica counts a request asks for, or why it is refused.
///
/// The argument check both replica-count operations share, and it runs BEFORE
/// the replication group is resolved: `ReplicationGroupId` is validated as an
/// argument by these two operations rather than looked up first, which is why an
/// id the account does not hold is answered `InvalidParameterValue` here while
/// the rest of the replication-group surface answers
/// `ReplicationGroupNotFoundFault`.
///
/// CC-02 growth bound: the replicas a request asks for are allocated per
/// request, so every count — `NewReplicaCount` and each
/// `ReplicaConfiguration` entry's — is checked against
/// [`MAX_REPLICAS_PER_NODE_GROUP`] here, before any member is created, and a
/// configuration entry can only name a shard the group already has (the
/// handlers resolve it), which [`MAX_NODE_GROUPS`] bounds.
///
/// `minimum` is the smallest count the CALLING operation can leave a shard with
/// — see [`INCREASE_REPLICA_COUNT_MIN`] and [`DECREASE_REPLICA_COUNT_MIN`] — and
/// is what the refusal states, so the bound a client is told is the one its call
/// is actually held to.
pub fn replica_count_targets(
    minimum: i32,
    new_replica_count: Option<i32>,
    replica_configuration: &[ConfigureShard],
) -> Result<ReplicaTargets, ReplicaCountRefusal> {
    if new_replica_count.is_some() && !replica_configuration.is_empty() {
        return Err(ReplicaCountRefusal::Combination(
            "NewReplicaCount cannot be combined with ReplicaConfiguration.".to_string(),
        ));
    }
    let in_range = |count: i32| -> Result<(), ReplicaCountRefusal> {
        if (minimum..=MAX_REPLICAS_PER_NODE_GROUP).contains(&count) {
            Ok(())
        } else {
            Err(ReplicaCountRefusal::Value(replica_count_range_message(
                minimum,
            )))
        }
    };
    if let Some(count) = new_replica_count {
        in_range(count)?;
        return Ok(ReplicaTargets::EveryShard(count));
    }
    if replica_configuration.is_empty() {
        return Err(ReplicaCountRefusal::Value(
            "Invalid value for NewReplicaCount: name either NewReplicaCount or \
             ReplicaConfiguration."
                .to_string(),
        ));
    }
    let mut targets: Vec<(String, i32)> = Vec::new();
    for shard in replica_configuration {
        in_range(shard.new_replica_count)?;
        if targets.iter().any(|(id, _)| id == &shard.node_group_id) {
            return Err(ReplicaCountRefusal::Value(format!(
                "Invalid value {} for ReplicaConfiguration.NodeGroupId: it is configured more \
                 than once.",
                shard.node_group_id
            )));
        }
        // "The number of PreferredAvailabilityZone values must equal the value
        // of NewReplicaCount plus 1 to account for the primary node."
        if let Some(zones) = shard.preferred_availability_zones.as_deref() {
            let expected = usize::try_from(shard.new_replica_count.saturating_add(1)).unwrap_or(1);
            if zones.len() != expected {
                return Err(ReplicaCountRefusal::Value(format!(
                    "Invalid value for ReplicaConfiguration.PreferredAvailabilityZones: {} zones \
                     were given for a node group of {expected} nodes.",
                    zones.len()
                )));
            }
        }
        // Outposts: the emulator provisions no Outpost capacity, so a replica
        // cannot be placed in one.
        if let Some(arn) = shard
            .preferred_outpost_arns
            .as_deref()
            .unwrap_or_default()
            .first()
        {
            return Err(ReplicaCountRefusal::Value(format!(
                "Invalid value {arn} for ReplicaConfiguration.PreferredOutpostArns: this \
                 emulator provisions no Outpost capacity."
            )));
        }
        targets.push((shard.node_group_id.clone(), shard.new_replica_count));
    }
    Ok(ReplicaTargets::PerShard(targets))
}

/// One user group, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct UserGroupRecord {
    pub user_group_id: String,
    pub engine: String,
    pub user_ids: Vec<String>,
    pub status: String,
    /// The status change the clock owes this group, if any.
    pub pending_status: Option<PendingTransition>,
    /// When the clock owes this group's REMOVAL from the store, set by the
    /// `DeleteUserGroup` that put it in [`STATUS_DELETING`]; `None` for a group
    /// nothing has deleted — the same rule a deleted user follows.
    pub pending_removal: Option<SystemTime>,
    /// The membership change the clock owes this group, as
    /// `ModifyUserGroup` accepted it: the ids to be added and the ids to be
    /// removed, which is what `UserGroup$PendingChanges` reports until
    /// [`Self::settle`] applies them to `user_ids` together with the status.
    pub pending_user_ids_to_add: Vec<String>,
    /// See [`Self::pending_user_ids_to_add`].
    pub pending_user_ids_to_remove: Vec<String>,
    /// The replication groups and serverless caches this group is attached to,
    /// which is what `UserGroup$ReplicationGroups` / `$ServerlessCaches` report.
    pub replication_group_ids: Vec<String>,
    pub serverless_cache_names: Vec<String>,
    pub region: String,
    pub arn: String,
    pub tags: Vec<(String, String)>,
}

impl UserGroupRecord {
    /// Materialise the change the clock owes this group: the status, and with it
    /// the membership change `ModifyUserGroup` accepted.
    ///
    /// The two fall due together — the group is `modifying` exactly while the
    /// change is pending — so one settle applies both and no read can see an
    /// `active` group that still has a change outstanding.
    pub fn settle(&mut self, now: SystemTime) {
        if !settle_status(&mut self.status, &mut self.pending_status, now) {
            return;
        }
        for user_id in ::std::mem::take(&mut self.pending_user_ids_to_add) {
            if !self.user_ids.contains(&user_id) {
                self.user_ids.push(user_id);
            }
        }
        let removed = ::std::mem::take(&mut self.pending_user_ids_to_remove);
        self.user_ids.retain(|member| !removed.contains(member));
    }

    /// True while the clock still owes this group a membership change, which is
    /// what `UserGroup$PendingChanges` reports.
    pub fn has_pending_changes(&self) -> bool {
        !self.pending_user_ids_to_add.is_empty() || !self.pending_user_ids_to_remove.is_empty()
    }

    /// This record as the modelled `UserGroup`.
    pub fn to_modeled(&self) -> CreateUserGroupOutput {
        CreateUserGroupOutput {
            user_group_id: Some(self.user_group_id.clone()),
            status: Some(self.status.clone()),
            engine: Some(self.engine.clone()),
            user_ids: Some(self.user_ids.clone()),
            minimum_engine_version: Some(USER_MIN_ENGINE_VERSION.to_string()),
            // "Returns the updates being applied to the user group": reported
            // only while `ModifyUserGroup` has left one outstanding, so a group
            // with nothing pending reports no `PendingChanges` at all.
            pending_changes: self.has_pending_changes().then(|| UserGroupPendingChanges {
                user_ids_to_add: Some(self.pending_user_ids_to_add.clone()),
                user_ids_to_remove: Some(self.pending_user_ids_to_remove.clone()),
            }),
            replication_groups: Some(self.replication_group_ids.clone()),
            serverless_caches: Some(self.serverless_cache_names.clone()),
            arn: Some(self.arn.clone()),
        }
    }
}

/// One member of a Global datastore, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct GlobalMemberRecord {
    pub replication_group_id: String,
    pub region: String,
    /// [`GLOBAL_ROLE_PRIMARY`] or [`GLOBAL_ROLE_SECONDARY`].
    pub role: String,
    pub automatic_failover: String,
    pub status: String,
}

/// One slice group of a Global datastore, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct GlobalNodeGroupRecord {
    pub global_node_group_id: String,
    pub slots: String,
}

/// The validated values a new Global datastore is built from.
///
/// A Global datastore has the configuration of the replication group it is built
/// around — its engine, engine version, node type, whether its keyspace is
/// partitioned and over how many slice groups, its encryption and its
/// auth-token setting — so `CreateGlobalReplicationGroup` reads every one of
/// those off that group rather than from the request.
#[derive(Debug, Clone)]
pub struct NewGlobalReplicationGroup {
    pub global_replication_group_id: String,
    pub description: Option<String>,
    pub region: String,
    pub engine: String,
    pub engine_version: String,
    pub cache_node_type: Option<String>,
    /// The replication group that accepts writes, which becomes the datastore's
    /// primary member.
    pub primary_replication_group_id: String,
    /// The primary member's `AutomaticFailover`, which is the primary
    /// replication group's own: see [`ReplicationGroupRecord::automatic_failover`].
    pub primary_automatic_failover: String,
    pub node_group_count: i32,
    pub cluster_enabled: bool,
    /// Whether the primary was created with an `AuthToken`; the token itself is
    /// a SECRET and is never stored. See
    /// [`CacheClusterRecord::auth_token_enabled`].
    pub auth_token_enabled: bool,
    pub transit_encryption_enabled: Option<bool>,
    pub at_rest_encryption_enabled: Option<bool>,
    /// The tags the create was given; see [`CacheClusterRecord::tags`].
    pub tags: Vec<(String, String)>,
    pub now: SystemTime,
}

/// One Global datastore, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct GlobalReplicationGroupRecord {
    pub global_replication_group_id: String,
    pub description: Option<String>,
    pub status: String,
    pub pending_status: Option<PendingTransition>,
    pub region: String,
    pub engine: String,
    pub engine_version: String,
    pub cache_node_type: Option<String>,
    pub members: Vec<GlobalMemberRecord>,
    pub cluster_enabled: bool,
    pub global_node_groups: Vec<GlobalNodeGroupRecord>,
    pub auth_token_enabled: bool,
    pub transit_encryption_enabled: Option<bool>,
    pub at_rest_encryption_enabled: Option<bool>,
    pub arn: String,
    pub tags: Vec<(String, String)>,
}

impl GlobalReplicationGroupRecord {
    /// A Global datastore with one primary member, in the status a just-created
    /// datastore has.
    ///
    /// It answers `creating` until the clock settles it at
    /// [`Self::settled_status`] — `primary-only` for a datastore that has only
    /// the primary it was built around, which is what that status means.
    pub fn new(new: NewGlobalReplicationGroup) -> Self {
        let mut record = Self {
            arn: global_replication_group_arn(&new.region, &new.global_replication_group_id),
            global_replication_group_id: new.global_replication_group_id,
            description: new.description,
            status: STATUS_CREATING.to_string(),
            pending_status: None,
            engine: new.engine,
            engine_version: new.engine_version,
            cache_node_type: new.cache_node_type,
            members: vec![GlobalMemberRecord {
                replication_group_id: new.primary_replication_group_id,
                region: new.region.clone(),
                role: GLOBAL_ROLE_PRIMARY.to_string(),
                automatic_failover: new.primary_automatic_failover,
                status: STATUS_ASSOCIATED.to_string(),
            }],
            cluster_enabled: new.cluster_enabled,
            global_node_groups: Vec::new(),
            auth_token_enabled: new.auth_token_enabled,
            transit_encryption_enabled: new.transit_encryption_enabled,
            at_rest_encryption_enabled: new.at_rest_encryption_enabled,
            region: new.region,
            tags: new.tags,
        };
        record.pending_status = transition_to(record.settled_status(), new.now);
        record.set_node_group_count(new.node_group_count);
        record
    }

    /// Materialise the status change the clock owes this datastore.
    pub fn settle(&mut self, now: SystemTime) {
        settle_status(&mut self.status, &mut self.pending_status, now);
    }

    /// Start the span after which the clock owes this datastore its settled
    /// status, leaving it in `status` until then.
    ///
    /// The settled status is [`STATUS_AVAILABLE`] while the datastore has a
    /// secondary member and [`STATUS_PRIMARY_ONLY`] once it has none, which is
    /// what that status means.
    pub fn begin_transition(&mut self, status: &str, now: SystemTime) {
        let settled = self.settled_status();
        self.status = status.to_string();
        self.pending_status = transition_to(settled, now);
    }

    /// The status this datastore settles at; see
    /// [`GlobalReplicationGroupRecord::begin_transition`].
    pub fn settled_status(&self) -> &'static str {
        if self
            .members
            .iter()
            .any(|member| member.role != GLOBAL_ROLE_PRIMARY)
        {
            STATUS_AVAILABLE
        } else {
            STATUS_PRIMARY_ONLY
        }
    }

    /// The member that accepts writes, if this datastore still has one.
    pub fn primary(&self) -> Option<&GlobalMemberRecord> {
        self.members
            .iter()
            .find(|member| member.role == GLOBAL_ROLE_PRIMARY)
    }

    /// Give this datastore `count` slice groups, keeping the ones it already has
    /// and re-slicing the keyspace evenly over the result.
    ///
    /// CC-02 growth bound: `count` is clamped to [`MAX_NODE_GROUPS`] here as
    /// well as refused above it by the callers, so this allocation cannot be
    /// driven past the shard quota by any request.
    pub fn set_node_group_count(&mut self, count: i32) {
        let target = usize::try_from(count.clamp(0, MAX_NODE_GROUPS)).unwrap_or(0);
        while self.global_node_groups.len() > target {
            self.global_node_groups.pop();
        }
        while self.global_node_groups.len() < target {
            let ordinal = self.global_node_groups.len() + 1;
            self.global_node_groups.push(GlobalNodeGroupRecord {
                global_node_group_id: format!(
                    "{}-{ordinal:04}",
                    self.global_replication_group_id
                ),
                slots: String::new(),
            });
        }
        self.rebalance_slots();
    }

    /// Re-slice the keyspace evenly over this datastore's slice groups, which is
    /// what `RebalanceSlotsInGlobalReplicationGroup` does.
    pub fn rebalance_slots(&mut self) {
        let ranges = even_slot_ranges(self.global_node_groups.len());
        for (group, range) in self.global_node_groups.iter_mut().zip(ranges) {
            group.slots = range;
        }
    }

    /// Keep only the slice groups `retain` names, then re-slice the keyspace.
    pub fn retain_node_groups(&mut self, retain: &[String]) {
        self.global_node_groups
            .retain(|group| retain.contains(&group.global_node_group_id));
        self.rebalance_slots();
    }

    /// The member `replication_group_id` has in `region`, if it is one.
    pub fn member(&self, replication_group_id: &str, region: &str) -> Option<&GlobalMemberRecord> {
        self.members.iter().find(|member| {
            member.replication_group_id == replication_group_id && member.region == region
        })
    }

    /// This record as the modelled `GlobalReplicationGroup`.
    ///
    /// `Members` is reported only when the caller asked for it
    /// (`DescribeGlobalReplicationGroups$ShowMemberInfo`: "Returns the list of
    /// members that comprise the Global datastore"); the mutating operations
    /// return the datastore they changed, members and all.
    pub fn to_modeled(&self, include_members: bool) -> GlobalReplicationGroup {
        GlobalReplicationGroup {
            global_replication_group_id: Some(self.global_replication_group_id.clone()),
            global_replication_group_description: self.description.clone(),
            status: Some(self.status.clone()),
            cache_node_type: self.cache_node_type.clone(),
            engine: Some(self.engine.clone()),
            engine_version: Some(self.engine_version.clone()),
            members: include_members.then(|| {
                self.members
                    .iter()
                    .map(|member| GlobalReplicationGroupMember {
                        replication_group_id: Some(member.replication_group_id.clone()),
                        replication_group_region: Some(member.region.clone()),
                        role: Some(member.role.clone()),
                        automatic_failover: Some(member.automatic_failover.clone()),
                        status: Some(member.status.clone()),
                    })
                    .collect()
            }),
            cluster_enabled: Some(self.cluster_enabled),
            global_node_groups: Some(
                self.global_node_groups
                    .iter()
                    .map(|group| GlobalNodeGroup {
                        global_node_group_id: Some(group.global_node_group_id.clone()),
                        slots: Some(group.slots.clone()),
                    })
                    .collect(),
            ),
            auth_token_enabled: Some(self.auth_token_enabled),
            transit_encryption_enabled: self.transit_encryption_enabled,
            at_rest_encryption_enabled: self.at_rest_encryption_enabled,
            arn: Some(self.arn.clone()),
        }
    }
}

/// The status a replication group has as a member of a Global datastore, which
/// is what `GlobalReplicationGroupMember$Status` reports.
pub const STATUS_ASSOCIATED: &str = "associated";

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
